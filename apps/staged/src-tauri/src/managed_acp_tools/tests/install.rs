use super::*;
use crate::managed_acp_tools::registry::{classify_npm_failure, NpmFailure};
use crate::managed_acp_tools::test_support::*;
use crate::managed_acp_tools::*;

/// The recorded install error carries npm's own account: the last
/// `npm error code` line and the detail lines after it, minus the `errno`
/// repeat and the log-file pointer, each clipped. Without an error block
/// nothing is appended.
#[test]
fn npm_error_summary_keeps_the_code_and_the_first_details() {
    let stderr = "npm warn deprecated foo@1.0.0: use bar\n\
        npm error code FETCH_ERROR\n\
        npm error errno FETCH_ERROR\n\
        npm error invalid json response body at https://registry.npmjs.org/x reason: Unexpected token '<'\n\
        npm error\n\
        npm error A complete log of this run can be found in: /tmp/x.log\n";
    assert_eq!(
        npm_error_summary(stderr).as_deref(),
        Some(
            "npm error code FETCH_ERROR; invalid json response body at \
             https://registry.npmjs.org/x reason: Unexpected token '<'"
        )
    );
    assert_eq!(
        classify_npm_failure(&npm_error_summary(stderr).unwrap()),
        NpmFailure::GatewayBlocked
    );

    // Older npm spelling, and more details than are kept.
    let stderr = "npm ERR! code E404\nnpm ERR! 404 Not Found - GET https://r/x\nnpm ERR! 404\nnpm ERR! 404 'x@latest' is not in this registry.\nnpm ERR! 404 note\nnpm ERR! 404 more\n";
    assert_eq!(
        npm_error_summary(stderr).as_deref(),
        Some("npm ERR! code E404; 404 Not Found - GET https://r/x; 404; 404 'x@latest' is not in this registry.")
    );

    assert_eq!(npm_error_summary("npm warn only\n"), None);
    assert_eq!(npm_error_summary(""), None);

    let long = format!("npm error code E1\nnpm error {}\n", "y".repeat(1000));
    let summary = npm_error_summary(&long).unwrap();
    assert!(summary.len() < 300, "{}", summary.len());
    assert!(summary.ends_with('…'));
}

/// The capture keeps the *newest* lines: npm's `npm error code …` block
/// comes last, after however many warnings preceded it.
#[test]
fn stderr_capture_keeps_the_tail() {
    let mut capture = String::new();
    let warning = format!("npm warn deprecated {}", "x".repeat(1000));
    for _ in 0..(NPM_STDERR_CAPTURE_LIMIT / warning.len() + 5) {
        push_captured_line(&mut capture, &warning);
    }
    push_captured_line(&mut capture, "npm error code FETCH_ERROR");
    push_captured_line(
        &mut capture,
        "npm error request to https://registry.npmjs.org/x failed: https://blocked.teams.cloudflare.com",
    );
    assert!(capture.len() <= NPM_STDERR_CAPTURE_LIMIT);
    assert!(capture.starts_with("npm warn"), "drops whole lines only");
    assert!(capture.ends_with("blocked.teams.cloudflare.com\n"));
    assert_eq!(classify_npm_failure(&capture), NpmFailure::GatewayBlocked);

    // A single line larger than the whole budget keeps its tail, cut on a
    // char boundary.
    let mut capture = String::new();
    push_captured_line(
        &mut capture,
        &format!("{}é end", "a".repeat(NPM_STDERR_CAPTURE_LIMIT)),
    );
    assert_eq!(capture.len(), NPM_STDERR_CAPTURE_LIMIT);
    assert!(capture.ends_with("é end\n"));
}

#[test]
fn shim_contents_execs_absolute_paths_and_quotes_spaces() {
    let contents = shim_contents(
        Path::new("/data/dir with spaces/packages/node/v1/plat/bin/node"),
        Path::new("/data/dir with spaces/packages/tools/claude-acp/node_modules/@scope/claude-acp/dist/index.js"),
    );
    assert!(contents.starts_with("#!/bin/sh\n"));
    assert!(contents.ends_with(
        "exec '/data/dir with spaces/packages/node/v1/plat/bin/node' '/data/dir with spaces/packages/tools/claude-acp/node_modules/@scope/claude-acp/dist/index.js' \"$@\"\n"
    ));
}

#[test]
fn write_shim_is_executable() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    write_shim(&bin_dir, "claude-agent-acp", "#!/bin/sh\nexec true\n").unwrap();
    let shim = bin_dir.join("claude-agent-acp");
    assert!(is_executable(&shim));
    assert_eq!(
        std::fs::read_to_string(&shim).unwrap(),
        "#!/bin/sh\nexec true\n"
    );
}

/// A fake managed-node `npm` that wipes its `--prefix` node_modules before
/// exiting — stands in for a floating upgrade npm aborts mid-reify. Proves
/// the swap fix: a live install this touched would be broken, so the test
/// installs into a staging prefix and the live tree must survive.
fn write_fake_node_with_destructive_npm(node_install_dir: &Path, exit_code: i32) {
    let bin = node_install_dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("node"), "#!/bin/sh\necho v9.9.9\n").unwrap();
    let npm = format!(
        "#!/bin/sh\nprefix=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--prefix\" ]; then prefix=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nrm -rf \"$prefix/node_modules\"\nexit {exit_code}\n"
    );
    std::fs::write(bin.join("npm"), npm).unwrap();
    use std::os::unix::fs::PermissionsExt;
    for name in ["node", "npm"] {
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[tokio::test]
async fn install_npm_tool_installs_shims_and_records_version() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();

    let template = dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    write_fixture_install(&template, &tool, "1.2.3");
    write_fake_node_with_npm(&node_install_dir, &template, 0);

    let lines = std::sync::Mutex::new(Vec::new());
    let on_line = |line: &str| lines.lock().unwrap().push(line.to_string());
    install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        None,
        super::validation::VALIDATION_TIMEOUT,
        &on_line,
    )
    .await
    .unwrap();

    let shim = shim_bin_dir(&packages_root).join(tool.binary);
    let entrypoint = npm_entrypoint(&tool_install_dir(&packages_root, tool.id), tool.package);
    assert!(is_executable(&shim));
    assert_eq!(
        std::fs::read_to_string(&shim).unwrap(),
        shim_contents(&node_binary(&node_install_dir), &entrypoint)
    );
    assert_eq!(
        read_state(&packages_root).tools.get(tool.id),
        Some(&InstalledToolPin {
            binary: tool.binary.to_string(),
            version: "1.2.3".to_string(),
            node_version: TEST_NODE_VERSION.to_string(),
        })
    );
    assert!(entrypoint.is_file());

    let recorded = lines.lock().unwrap().clone();
    assert!(recorded
        .iter()
        .any(|line| line.contains("added 3 packages")));
    assert!(recorded.iter().any(|line| line.contains("1.2.3 is ready")));
}

#[tokio::test]
async fn failed_npm_install_writes_no_shim_and_no_state() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();

    let template = dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    write_fixture_install(&template, &tool, "1.2.3");
    write_fake_node_with_npm(&node_install_dir, &template, 7);

    let error = install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        None,
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
    assert!(!shim_bin_dir(&packages_root).join(tool.binary).exists());
    assert!(read_state(&packages_root).tools.is_empty());
}

#[tokio::test]
async fn install_without_entrypoint_fails_incomplete_before_shims() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();

    // A clean npm exit that produced no bridge entrypoint (empty template).
    let template = dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    write_fake_node_with_npm(&node_install_dir, &template, 0);

    let error = install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        None,
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ManagedToolError::Incomplete(_)), "{error}");
    assert!(!shim_bin_dir(&packages_root).join(tool.binary).exists());
    assert!(read_state(&packages_root).tools.is_empty());
}

#[tokio::test]
async fn failed_upgrade_preserves_the_previous_install() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();

    // A healthy previously-installed bridge: tree + shim + state at 1.2.3.
    write_installed_tool(&packages_root, &node_install_dir, &tool);
    let install_dir = tool_install_dir(&packages_root, tool.id);
    let entrypoint = npm_entrypoint(&install_dir, tool.package);
    let shim = shim_bin_dir(&packages_root).join(tool.binary);
    let shim_before = std::fs::read_to_string(&shim).unwrap();

    // An upgrade whose npm destroys its --prefix tree and then fails. It
    // only ever touches the staging prefix, so the live install survives.
    write_fake_node_with_destructive_npm(&node_install_dir, 7);
    let error = install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        None,
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
    assert!(
        entrypoint.is_file(),
        "live entrypoint clobbered by failed upgrade"
    );
    assert_eq!(std::fs::read_to_string(&shim).unwrap(), shim_before);
    assert_eq!(
        read_state(&packages_root)
            .tools
            .get(tool.id)
            .map(|pin| pin.version.clone()),
        Some("1.2.3".to_string())
    );
    // No scratch dirs are left behind for the next reconcile to trip over.
    assert!(!staging_install_dir(&packages_root, tool.id).exists());
    assert!(!install_dir.with_extension("old").exists());
    // And the version Doctor reads is still the working install's.
    assert_eq!(
        installed_tool_version(&packages_root, &tool).as_deref(),
        Some("1.2.3")
    );
}

/// A managed update writes only under the packages root: nothing lands in
/// a global prefix, and the fixture's neighbours are left alone.
#[tokio::test]
async fn install_touches_only_the_packages_root() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();

    let template = dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    write_fixture_install(&template, &tool, "1.2.4");
    write_fake_node_with_npm(&node_install_dir, &template, 0);
    let outside = dir.path().join("global-bin");
    std::fs::create_dir_all(&outside).unwrap();

    install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        None,
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap();

    let mut entries: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["global-bin", "packages", "template"]);
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    assert_eq!(
        installed_tool_version(&packages_root, &tool).as_deref(),
        Some("1.2.4")
    );
}

/// A fake managed-node `npm` standing in for a laptop whose network
/// gateway blocks `registry.npmjs.org`: without `--registry` it prints
/// npm's block-page failure and exits 1; with one it performs the install
/// from `template`. Every invocation's argv is appended to `argv_log`.
fn write_fake_node_with_gateway_blocked_npm(
    node_install_dir: &Path,
    template: &Path,
    argv_log: &Path,
    stderr_without_registry: &str,
) {
    let bin = node_install_dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("node"), "#!/bin/sh\necho v9.9.9\n").unwrap();
    let npm = format!(
        "#!/bin/sh\necho \"$*\" >> '{log}'\nprefix=\"\"\nregistry=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--prefix\" ]; then prefix=\"$arg\"; fi\n  if [ \"$prev\" = \"--registry\" ]; then registry=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -z \"$registry\" ]; then\n  printf '%s\\n' '{stderr}' >&2\n  exit 1\nfi\ncp -R '{template}/.' \"$prefix/\"\necho \"added 3 packages\"\nexit 0\n",
        log = argv_log.display(),
        stderr = stderr_without_registry.replace('\'', "'\\''"),
        template = template.display(),
    );
    std::fs::write(bin.join("npm"), npm).unwrap();
    use std::os::unix::fs::PermissionsExt;
    for name in ["node", "npm"] {
        std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

const GATEWAY_BLOCK_STDERR: &str = "npm error code FETCH_ERROR\nnpm error errno FETCH_ERROR\nnpm error invalid json response body at https://registry.npmjs.org/@agentclientprotocol%2fclaude-agent-acp reason: Unexpected token < in JSON at position 0";

fn logged_argv(argv_log: &Path) -> Vec<String> {
    std::fs::read_to_string(argv_log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// No override, a compiled-in fallback, and npm failing on a gateway
/// block page: the install is retried once through the fallback from a
/// fresh staging prefix, and succeeds.
#[tokio::test]
async fn gateway_blocked_install_retries_once_through_the_fallback_registry() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();
    let template = dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    write_fixture_install(&template, &tool, "1.2.5");
    let argv_log = dir.path().join("npm-argv.log");
    write_fake_node_with_gateway_blocked_npm(
        &node_install_dir,
        &template,
        &argv_log,
        GATEWAY_BLOCK_STDERR,
    );

    let lines = std::sync::Mutex::new(Vec::new());
    let on_line = |line: &str| lines.lock().unwrap().push(line.to_string());
    install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        Some("https://fallback.example.test/npm/"),
        super::validation::VALIDATION_TIMEOUT,
        &on_line,
    )
    .await
    .unwrap();

    let argv = logged_argv(&argv_log);
    assert_eq!(argv.len(), 2, "{argv:?}");
    assert!(!argv[0].contains("--registry"), "{}", argv[0]);
    assert!(
        argv[1].contains("--registry https://fallback.example.test/npm/"),
        "{}",
        argv[1]
    );
    assert!(
        argv[1].contains("--@agentclientprotocol:registry=https://fallback.example.test/npm/"),
        "{}",
        argv[1]
    );
    assert_eq!(
        installed_tool_version(&packages_root, &tool).as_deref(),
        Some("1.2.5")
    );
    let recorded = lines.lock().unwrap().clone();
    assert!(
        recorded
            .iter()
            .any(|line| line.contains("retrying through https://fallback.example.test/npm/")),
        "{recorded:?}"
    );
    assert!(!staging_install_dir(&packages_root, tool.id).exists());
}

/// When the fallback attempt fails too, the surfaced error still says why
/// the fallback was contacted: the default registry was gateway-blocked.
#[tokio::test]
async fn gateway_blocked_install_names_both_failures_when_the_fallback_fails() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();
    // Fails whatever registry it is given: a block page without one, a
    // plain exit 7 with one.
    let bin = node_install_dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_executable(&bin.join("node"), "#!/bin/sh\necho v9.9.9\n");
    write_executable(
        &bin.join("npm"),
        &format!(
            "#!/bin/sh\ncase \" $* \" in\n  *\" --registry \"*) echo 'npm error code E500' >&2; exit 7 ;;\nesac\nprintf '%s\\n' '{}' >&2\nexit 1\n",
            GATEWAY_BLOCK_STDERR.replace('\'', "'\\''")
        ),
    );

    let error = install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        Some("https://fallback.example.test/npm/"),
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
    let message = error.to_string();
    // The retry's own `npm error code` line rides along, so the recorded
    // error says what the fallback answered, not just that it failed.
    assert_eq!(
        message,
        "npm install failed: npm default registry (registry.npmjs.org) was blocked by a \
         network gateway; fallback https://fallback.example.test/npm/ also failed: \
         npm install exited with exit status: 7 (registry: https://fallback.example.test/npm/): \
         npm error code E500"
    );
    assert!(
        message.contains(
            "fallback https://fallback.example.test/npm/ also failed: npm install exited with"
        ),
        "{message}"
    );
    assert!(
        message.contains("(registry: https://fallback.example.test/npm/)"),
        "{message}"
    );
    assert!(!staging_install_dir(&packages_root, tool.id).exists());
}

/// npm runs under the env snapshot, not the Tauri process env: a registry
/// the user's shell rc exports reaches the install exactly as it reaches
/// the freshness probe, while the managed node bin leads PATH and the
/// private cache outranks whatever cache the snapshot named.
#[tokio::test]
async fn install_runs_npm_under_the_env_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();
    let template = dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    write_fixture_install(&template, &tool, "1.2.3");
    write_fake_node_with_npm(&node_install_dir, &template, 0);
    // The fake npm also dumps the env it saw.
    let env_log = dir.path().join("npm-env.log");
    let npm = node_install_dir.join("bin").join("npm");
    let script = std::fs::read_to_string(&npm).unwrap();
    write_executable(
        &npm,
        &script.replacen(
            "#!/bin/sh\n",
            &format!("#!/bin/sh\nenv > '{}'\n", env_log.display()),
            1,
        ),
    );

    let mut env_vars = install_env();
    env_vars.push((
        "NPM_CONFIG_REGISTRY".to_string(),
        "https://mine.example.test/npm/".to_string(),
    ));
    env_vars.push((
        "npm_config_registry".to_string(),
        "https://mine.example.test/npm/".to_string(),
    ));
    env_vars.push((
        "NPM_CONFIG_CACHE".to_string(),
        "/somewhere/else/cache".to_string(),
    ));
    env_vars.push(("STAGED_TEST_SNAPSHOT_ONLY".to_string(), "1".to_string()));
    install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &env_vars,
        None,
        None,
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap();

    let seen: std::collections::HashMap<String, String> = std::fs::read_to_string(&env_log)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    assert_eq!(
        seen.get("NPM_CONFIG_REGISTRY").map(String::as_str),
        Some("https://mine.example.test/npm/")
    );
    assert_eq!(
        seen.get("npm_config_registry").map(String::as_str),
        Some("https://mine.example.test/npm/")
    );
    assert_eq!(
        seen.get("STAGED_TEST_SNAPSHOT_ONLY").map(String::as_str),
        Some("1"),
        "snapshot-only vars reach npm"
    );
    let expected_cache = packages_root.join("npm-prefix").join("cache");
    assert_eq!(
        seen.get("NPM_CONFIG_CACHE").map(Path::new),
        Some(expected_cache.as_path()),
        "the managed cache overlay wins over the snapshot's"
    );
    let first_on_path = std::env::split_paths(seen.get("PATH").unwrap()).next();
    assert_eq!(
        first_on_path.as_deref(),
        Some(node_install_dir.join("bin").as_path())
    );
}

/// An explicit `STAGED_NPM_REGISTRY` is never second-guessed, and a
/// failure that is not a gateway block page is not retried either; both
/// surface the first failure, naming the registry npm used.
#[tokio::test]
async fn install_does_not_retry_with_an_override_or_for_other_failures() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let tool = test_tool();
    let template = dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    write_fixture_install(&template, &tool, "1.2.5");
    let fallback = Some("https://fallback.example.test/npm/");

    // A gateway block page, but no fallback compiled in: plain failure.
    let argv_log = dir.path().join("no-fallback.log");
    write_fake_node_with_gateway_blocked_npm(
        &node_install_dir,
        &template,
        &argv_log,
        GATEWAY_BLOCK_STDERR,
    );
    let error = install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        None,
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("(registry: npm default (per npm config))"),
        "{error}"
    );
    assert_eq!(logged_argv(&argv_log).len(), 1);

    // A 404 from whichever registry npm used: a different registry cannot
    // fix that, so no retry even with a fallback available.
    let argv_log = dir.path().join("not-found.log");
    write_fake_node_with_gateway_blocked_npm(
        &node_install_dir,
        &template,
        &argv_log,
        "npm error code E404\nnpm error 404 Not Found - GET https://mirror.example.test/npm/@agentclientprotocol%2fclaude-agent-acp",
    );
    let error = install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        None,
        fallback,
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
    assert_eq!(logged_argv(&argv_log).len(), 1);

    // An explicit override: the one and only call carries it (the fake
    // succeeds on any `--registry`), and the fallback is never consulted.
    let argv_log = dir.path().join("override.log");
    write_fake_node_with_gateway_blocked_npm(
        &node_install_dir,
        &template,
        &argv_log,
        GATEWAY_BLOCK_STDERR,
    );
    install_npm_tool(
        &packages_root,
        &node_install_dir,
        TEST_NODE_VERSION,
        &tool,
        &install_env(),
        Some("https://mine.example.test/npm/"),
        fallback,
        super::validation::VALIDATION_TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap();
    let argv = logged_argv(&argv_log);
    assert_eq!(argv.len(), 1, "{argv:?}");
    assert!(
        argv[0].contains("--registry https://mine.example.test/npm/"),
        "{}",
        argv[0]
    );
    assert!(
        argv[0].contains("--@agentclientprotocol:registry=https://mine.example.test/npm/"),
        "{}",
        argv[0]
    );
}

/// The live tree's `package.json` is the authority for the installed ACP
/// version; `state.json` only stands in when that tree cannot be read.
#[test]
fn installed_tool_version_prefers_the_live_package_over_state() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path();
    let tool = test_tool();

    // Nothing installed, no state: unknown.
    assert_eq!(installed_tool_version(packages_root, &tool), None);

    // State alone (tree missing or unreadable): the recorded version.
    let mut state = ManagedToolsState::default();
    state.tools.insert(
        tool.id.to_string(),
        InstalledToolPin {
            binary: tool.binary.to_string(),
            version: "1.2.3".to_string(),
            node_version: TEST_NODE_VERSION.to_string(),
        },
    );
    write_state(packages_root, &state).unwrap();
    assert_eq!(
        installed_tool_version(packages_root, &tool).as_deref(),
        Some("1.2.3")
    );

    // A live tree at another version wins over stale state.
    write_fixture_install(&tool_install_dir(packages_root, tool.id), &tool, "1.3.0");
    assert_eq!(
        installed_tool_version(packages_root, &tool).as_deref(),
        Some("1.3.0")
    );

    // An empty recorded version (unreadable package.json at install
    // time) is not a version.
    std::fs::remove_dir_all(tool_install_dir(packages_root, tool.id)).unwrap();
    state.tools.get_mut(tool.id).unwrap().version.clear();
    write_state(packages_root, &state).unwrap();
    assert_eq!(installed_tool_version(packages_root, &tool), None);
}
