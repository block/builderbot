//! Shared by the Staged MCP on Blox tests: a local `node`, a local stand-in
//! for `ws exec`, and a driver that talks to the script the way an agent does.

use std::io::Write;
use std::process::{Command, Stdio};

/// The script in the source tree; [`super::SCRIPT_SOURCE`] embeds the same file.
pub(super) const SCRIPT_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/remote/staged-mcp.cjs"
);

/// The local `node`, or `None` when it is not installed (the subprocess tests
/// then skip themselves).
pub(super) fn node() -> Option<String> {
    let output = Command::new("sh")
        .args(["-c", "command -v node"])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !path.is_empty()).then_some(path)
}

/// Run `args` locally with `HOME` pointed at `home`, mirroring what `ws
/// exec` does on the workstation.
pub(super) fn local_exec(
    home: &std::path::Path,
) -> impl Fn(&[&str]) -> Result<String, String> + '_ {
    move |args: &[&str]| {
        let output = Command::new(args[0])
            .args(&args[1..])
            .env("HOME", home)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }
}

/// Drive the script as the agent would, for session `sess-1` and turn
/// `turn-1`: one JSON-RPC request per line on stdin, one response per line on
/// stdout. Returns the parsed responses.
pub(super) fn run_script(
    home: &std::path::Path,
    node: &str,
    requests: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let lines: Vec<String> = requests.iter().map(ToString::to_string).collect();
    run_script_lines(home, node, &lines)
}

/// [`run_script`] with raw stdin lines, for input that is not valid JSON.
pub(super) fn run_script_lines(
    home: &std::path::Path,
    node: &str,
    lines: &[String],
) -> Vec<serde_json::Value> {
    run_node_lines(SCRIPT_PATH, &[], home, node, lines).responses
}

/// What a run of the script (or a driver around it) produced.
pub(super) struct ScriptRun {
    /// The parsed stdout frames, in the order they were written.
    pub(super) responses: Vec<serde_json::Value>,
    /// Everything the script logged.
    pub(super) stderr: String,
}

/// Run `node <script> <script_args...> sess-1 turn-1` with `HOME` at `home`,
/// feeding `lines` on stdin. `script` is the server itself or a driver that
/// requires it; a driver takes its own arguments before the session and turn.
pub(super) fn run_node_lines(
    script: &str,
    script_args: &[&str],
    home: &std::path::Path,
    node: &str,
    lines: &[String],
) -> ScriptRun {
    let mut child = Command::new(node)
        .arg(script)
        .args(script_args)
        .args(["sess-1", "turn-1"])
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the Staged MCP script");
    {
        let mut stdin = child.stdin.take().unwrap();
        for line in lines {
            writeln!(stdin, "{line}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "the Staged MCP script exited {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let responses = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect();
    ScriptRun {
        responses,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}
