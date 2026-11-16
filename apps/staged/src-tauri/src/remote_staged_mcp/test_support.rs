//! Shared by the Staged MCP on Blox tests: a local `node`, a local stand-in
//! for `ws exec`, and a driver that talks to the script the way an agent does.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long [`run_node_lines`] waits for the script to exit after stdin
/// closes. The script must end itself when stdin closes even while a tool
/// holds a live handle; a run that outlives this is killed and fails.
const SCRIPT_EXIT_TIMEOUT: Duration = Duration::from_secs(60);

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
pub(crate) fn local_exec(
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

fn read_to_end_in_thread<R: Read + Send + 'static>(
    mut pipe: R,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).unwrap();
        bytes
    })
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
/// The script must exit 0 within [`SCRIPT_EXIT_TIMEOUT`] of stdin closing.
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
    // Drain both pipes on threads so a chatty script cannot block on a full
    // pipe while the main thread waits for it to exit.
    let stdout_reader = read_to_end_in_thread(child.stdout.take().unwrap());
    let stderr_reader = read_to_end_in_thread(child.stderr.take().unwrap());
    {
        let mut stdin = child.stdin.take().unwrap();
        for line in lines {
            writeln!(stdin, "{line}").unwrap();
        }
    }
    let closed_at = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if closed_at.elapsed() > SCRIPT_EXIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let stderr = String::from_utf8_lossy(&stderr_reader.join().unwrap()).into_owned();
            panic!(
                "the Staged MCP script was still running {SCRIPT_EXIT_TIMEOUT:?} after stdin closed: {stderr}"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout_reader.join().unwrap();
    let stderr = String::from_utf8_lossy(&stderr_reader.join().unwrap()).into_owned();
    assert!(
        status.success(),
        "the Staged MCP script exited {status:?}: {stderr}"
    );
    let responses = String::from_utf8(stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect();
    ScriptRun { responses, stderr }
}
