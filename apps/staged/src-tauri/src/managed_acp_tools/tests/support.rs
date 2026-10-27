//! Fixtures shared by the managed-tool test modules: a fake managed Node
//! runtime whose `npm` copies a template install into `--prefix`, fixture
//! package trees, and the env snapshot installs run under.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::install::{node_binary, npm_entrypoint, package_dir, shim_contents, write_shim};
use super::*;

pub(super) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub(super) fn test_tool() -> ManagedTool {
    ManagedTool {
        id: "claude-acp",
        binary: "claude-agent-acp",
        package: "@agentclientprotocol/claude-agent-acp",
    }
}

pub(super) const TEST_NODE_VERSION: &str = "v9.9.9";

/// The env snapshot the fixture installs run under: enough of the test
/// process env for a `#!/bin/sh` fake npm to find `cp` and its home.
pub(super) fn install_env() -> Vec<(String, String)> {
    ["PATH", "HOME"]
        .into_iter()
        .filter_map(|key| Some((key.to_string(), std::env::var(key).ok()?)))
        .collect()
}

pub(super) fn write_json(path: &Path, value: &serde_json::Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

/// A fixture install tree: the package's `package.json` (with the resolved
/// version) and its `dist/index.js` entrypoint.
pub(super) fn write_fixture_install(install_dir: &Path, tool: &ManagedTool, version: &str) {
    write_json(
        &package_dir(install_dir, tool.package).join("package.json"),
        &serde_json::json!({ "name": tool.package, "version": version }),
    );
    let entrypoint = npm_entrypoint(install_dir, tool.package);
    std::fs::create_dir_all(entrypoint.parent().unwrap()).unwrap();
    std::fs::write(&entrypoint, "// bridge\n").unwrap();
}

/// A fake managed-node install dir whose `npm` copies a pre-built fixture
/// tree into the `--prefix` dir, standing in for a real floating install.
pub(super) fn write_fake_node_with_npm(node_install_dir: &Path, template: &Path, exit_code: i32) {
    let bin = node_install_dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("node"), "#!/bin/sh\necho v9.9.9\n").unwrap();
    let npm = format!(
        "#!/bin/sh\nprefix=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--prefix\" ]; then prefix=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncp -R '{}/.' \"$prefix/\"\necho \"added 3 packages\"\nexit {exit_code}\n",
        template.display()
    );
    std::fs::write(bin.join("npm"), npm).unwrap();
    use std::os::unix::fs::PermissionsExt;
    for name in ["node", "npm"] {
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Env var [`wait_until_executable`] sets on its probe run.
const EXEC_PROBE_ENV: &str = "STAGED_EXEC_PROBE";
/// Line [`with_exec_probe_guard`] puts right after the shebang: a probe run
/// exits before reaching any of the fixture's own lines.
const EXEC_PROBE_GUARD: &str = "[ -z \"$STAGED_EXEC_PROBE\" ] || exit 0\n";

/// `contents` with [`EXEC_PROBE_GUARD`] inserted after its shebang line, so
/// [`wait_until_executable`] can exec the script without running its body.
pub(super) fn with_exec_probe_guard(contents: &str) -> String {
    let (shebang, body) = contents
        .split_once('\n')
        .filter(|(shebang, _)| shebang.starts_with("#!"))
        .expect("fixture script must start with a shebang line");
    format!("{shebang}\n{EXEC_PROBE_GUARD}{body}")
}

/// Writes `contents` (behind [`with_exec_probe_guard`]) to `path` as a 0755
/// script and returns only once it can actually be exec'd (see
/// [`wait_until_executable`]).
pub(super) fn write_executable(path: &Path, contents: &str) {
    std::fs::write(path, with_exec_probe_guard(contents)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    wait_until_executable(path);
}

/// Returns only once the freshly written script at `path` can actually be
/// exec'd. On Linux, a test on another thread that forks while our write
/// handle is open inherits it until its own exec, and exec of a file
/// anyone still holds open for writing fails with `ETXTBSY` — which the
/// code under test would faithfully report and fail the test. Probe-run
/// the script until that window has passed. The script must carry
/// [`EXEC_PROBE_GUARD`]: the probe runs it with a cleared env plus
/// `STAGED_EXEC_PROBE=1` and waits for it, so a probe run is a guaranteed
/// no-op. (Killing an unguarded script instead races: on Linux `sh` can
/// already be running its first lines — e.g. a fixture's `env > log` —
/// under the test process env by the time the kill lands.)
pub(super) fn wait_until_executable(path: &Path) {
    let contents = std::fs::read_to_string(path).unwrap();
    assert!(
        contents.contains(EXEC_PROBE_GUARD),
        "{} lacks the exec-probe guard; write it via with_exec_probe_guard",
        path.display()
    );
    for _ in 0..1000 {
        match std::process::Command::new(path)
            .env_clear()
            .env(EXEC_PROBE_ENV, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let status = child.wait().unwrap();
                assert!(
                    status.success(),
                    "exec probe of {} exited {status}",
                    path.display()
                );
                return;
            }
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("{} is not executable: {error}", path.display()),
        }
    }
    panic!("{} stayed text-busy", path.display());
}

/// Lay down a complete healthy install (tree + shim + state) for `tool`.
pub(super) fn write_installed_tool(
    packages_root: &Path,
    node_install_dir: &Path,
    tool: &ManagedTool,
) {
    let install_dir = tool_install_dir(packages_root, tool.id);
    write_fixture_install(&install_dir, tool, "1.2.3");
    let entrypoint = npm_entrypoint(&install_dir, tool.package);
    write_shim(
        &shim_bin_dir(packages_root),
        tool.binary,
        &shim_contents(&node_binary(node_install_dir), &entrypoint),
    )
    .unwrap();
    let mut state = read_state(packages_root);
    state.tools.insert(
        tool.id.to_string(),
        InstalledToolPin {
            binary: tool.binary.to_string(),
            version: "1.2.3".to_string(),
            node_version: TEST_NODE_VERSION.to_string(),
        },
    );
    write_state(packages_root, &state).unwrap();
}

/// A packages root with the pinned Node runtime dir plus a superseded
/// version left over from before a Node pin bump. Returns
/// `(packages_root, pinned_dir, superseded_dir)`.
pub(super) fn write_node_bump_leftovers(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let packages_root = dir.join("packages");
    let node_root = packages_root.join("node");
    let pinned_dir = managed_node::pinned_install_dir(&node_root).unwrap();
    let superseded_dir = node_root.join("v0.0.1").join("plat");
    std::fs::create_dir_all(&pinned_dir).unwrap();
    std::fs::create_dir_all(&superseded_dir).unwrap();
    (packages_root, pinned_dir, superseded_dir)
}
