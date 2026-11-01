//! Workstation-side `write_note` for remote (Blox) note sessions.
//!
//! A remote agent cannot reach the laptop's loopback MCP servers, and the
//! laptop may not even be connected when the agent finishes. So a remote note
//! session gets the same `write_note` tool from a dependency-free stdio MCP
//! helper (`resources/remote/staged-note-mcp.cjs`, Node standard library only, CommonJS
//! so it runs under any `package.json` module type; Blox images ship `node`)
//! that Staged uploads to the workstation and injects into `session/new` as
//! [`McpServer::Stdio`]. Each call writes the note atomically into a per-session
//! outbox under the workstation home:
//!
//! ```text
//! ~/.staged/bin/staged-note-mcp.cjs          the helper (checksum-gated upload)
//! ~/.staged/bin/staged-note-mcp.sha256      SHA-256 of the installed helper
//! ~/.staged/sessions/<session-id>/note.md   body (markdown, without the title)
//! ~/.staged/sessions/<session-id>/meta.json title, suggested next steps, turn id
//! ~/.staged/sessions/<session-id>/done      marker: a complete note is present
//! ```
//!
//! After the turn — or on the dead-owner sweep, when the laptop lost the
//! session — the collector pulls the outbox over `ws exec` and turns it into
//! the same [`NoteSubmission`] the local tool produces, so the post-completion
//! hooks (title resolution, media ingestion) do not know which transport
//! served the call.
//!
//! Everything that talks to the workstation goes through a caller-provided
//! runner (`&dyn Fn(&[&str]) -> Result<String, String>`), the same shape
//! [`crate::session_commands::write_bytes_to_remote_with`] takes, so the
//! command sequences are unit-testable without a workspace and the shell
//! scripts can be exercised locally against `sh`.

use std::path::PathBuf;

use acp_client::{McpServer, McpServerStdio};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::note_tool::{
    NoteSubmission, NoteSubmissionSource, SuggestedNextSteps, WriteNoteParams, NOTE_MCP_SERVER_NAME,
};

/// The helper script, embedded so the running build always installs the
/// version it was built with.
pub(crate) const HELPER_SOURCE: &str = include_str!("../resources/remote/staged-note-mcp.cjs");

const HELPER_FILE_NAME: &str = "staged-note-mcp.cjs";
const HELPER_CHECKSUM_FILE_NAME: &str = "staged-note-mcp.sha256";

/// Remote command runner: `args` run inside the workspace, stdout comes back.
pub(crate) type RemoteExec<'a> = &'a dyn Fn(&[&str]) -> Result<String, String>;

/// Build a [`RemoteExec`]-compatible closure for a Blox workspace.
pub(crate) fn workspace_exec(
    workspace_name: &str,
) -> impl Fn(&[&str]) -> Result<String, String> + '_ {
    move |args| {
        crate::blox::ws_exec(workspace_name, crate::blox::ExecMode::Transient, args)
            .map_err(|e| e.to_string())
    }
}

/// SHA-256 (lowercase hex) of the embedded helper; the workstation keeps the
/// checksum of what it has installed so an unchanged helper is never
/// re-uploaded.
pub(crate) fn helper_checksum() -> String {
    let mut hasher = Sha256::new();
    hasher.update(HELPER_SOURCE.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Session ids are interpolated into remote shell scripts and file paths, so
/// only the characters a Staged uuid can contain are accepted.
pub(crate) fn validate_session_id(session_id: &str) -> Result<(), String> {
    let ok = !session_id.is_empty()
        && session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "session id {session_id:?} is not safe to use in a remote path"
        ))
    }
}

/// Where the helper lives on the workstation after [`install_helper`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteNoteHelper {
    /// Absolute path of the workstation's `node`.
    pub node: String,
    /// Absolute path of the installed helper script.
    pub helper_path: String,
}

/// What the install probe reports about the workstation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallProbe {
    pub home: String,
    pub installed_checksum: Option<String>,
    pub node: Option<String>,
}

/// One round trip that creates `~/.staged/bin` and prints, one per line, the
/// home directory, the checksum of the installed helper (empty when absent)
/// and the path of `node` (empty when absent). Always exits 0 so `ws exec`
/// returns the output.
pub(crate) fn install_probe_script() -> String {
    format!(
        "d=\"$HOME/.staged/bin\"; mkdir -p \"$d\" && chmod 700 \"$HOME/.staged\" \"$d\"; \
         printf '%s\\n' \"$HOME\"; \
         printf '%s\\n' \"$(cat \"$d/{HELPER_CHECKSUM_FILE_NAME}\" 2>/dev/null || true)\"; \
         printf '%s\\n' \"$(command -v node 2>/dev/null || true)\"; exit 0"
    )
}

pub(crate) fn parse_install_probe(output: &str) -> Result<InstallProbe, String> {
    let mut lines = output.lines().map(str::trim);
    let home = lines
        .next()
        .filter(|home| home.starts_with('/'))
        .ok_or_else(|| format!("install probe returned no home directory: {output:?}"))?
        .to_string();
    let installed_checksum = lines
        .next()
        .map(str::to_string)
        .filter(|sum| !sum.is_empty());
    let node = lines
        .next()
        .map(str::to_string)
        .filter(|path| path.starts_with('/'));
    Ok(InstallProbe {
        home,
        installed_checksum,
        node,
    })
}

/// Make sure the helper is installed on the workstation and return where it
/// is. Uploads the script (base64 over `ws exec`, temp + rename) only when the
/// recorded checksum differs from the embedded helper's, then records the new
/// checksum. Fails when the workstation has no `node`.
pub(crate) fn install_helper(exec: RemoteExec) -> Result<RemoteNoteHelper, String> {
    let probe = exec(&["sh", "-c", &install_probe_script()])
        .map_err(|e| format!("failed to probe the workstation for the note helper: {e}"))?;
    let probe = parse_install_probe(&probe)?;
    let Some(node) = probe.node else {
        return Err(
            "the workstation has no `node`, which the remote write_note helper needs".to_string(),
        );
    };
    let bin_dir = format!("{}/.staged/bin", probe.home);
    let helper_path = format!("{bin_dir}/{HELPER_FILE_NAME}");
    let checksum = helper_checksum();

    if probe.installed_checksum.as_deref() != Some(checksum.as_str()) {
        crate::session_commands::write_bytes_to_remote_with(
            exec,
            HELPER_SOURCE.as_bytes(),
            &helper_path,
        )
        .map_err(|e| format!("failed to upload the remote write_note helper: {e}"))?;
        let record = format!(
            "chmod 700 '{helper_path}' && printf '%s' '{checksum}' > '{bin_dir}/{HELPER_CHECKSUM_FILE_NAME}'"
        );
        exec(&["sh", "-c", &record])
            .map_err(|e| format!("failed to record the remote write_note helper checksum: {e}"))?;
        log::info!("Installed remote write_note helper at {helper_path} ({checksum})");
    } else {
        log::debug!("Remote write_note helper at {helper_path} is current ({checksum})");
    }

    Ok(RemoteNoteHelper { node, helper_path })
}

/// The outbox directory as a shell expression (double-quoted `$HOME`), for
/// use inside `sh -c` scripts. The helper derives the same path from
/// `os.path.expanduser("~")`.
fn outbox_dir_expr(session_id: &str) -> String {
    format!("\"$HOME/.staged/sessions/{session_id}\"")
}

/// Empty the session's outbox so a stale note from an earlier run can never be
/// mistaken for this run's. Runs at session start, while the laptop is
/// connected. Always exits 0.
pub(crate) fn clear_outbox_script(session_id: &str) -> String {
    let dir = outbox_dir_expr(session_id);
    format!("d={dir}; rm -f \"$d/note.md\" \"$d/meta.json\" \"$d/done\"; exit 0")
}

pub(crate) fn clear_outbox(exec: RemoteExec, session_id: &str) -> Result<(), String> {
    validate_session_id(session_id)?;
    exec(&["sh", "-c", &clear_outbox_script(session_id)])
        .map(|_| ())
        .map_err(|e| format!("failed to clear the remote note outbox: {e}"))
}

/// The stdio MCP server entry for `session/new`: the workstation's `node`
/// running the helper for this session and turn.
pub(crate) fn stdio_mcp_server(
    helper: &RemoteNoteHelper,
    session_id: &str,
    turn_id: &str,
) -> McpServer {
    McpServer::Stdio(
        McpServerStdio::new(NOTE_MCP_SERVER_NAME, PathBuf::from(&helper.node)).args(vec![
            helper.helper_path.clone(),
            session_id.to_string(),
            turn_id.to_string(),
        ]),
    )
}

/// Print the outbox as three lines: `done`/`pending`, then `meta.json` and
/// `note.md` each as one base64 line (empty when the file is missing). Base64
/// keeps arbitrary markdown — including lines that look like our framing —
/// out of the line protocol. Always exits 0.
pub(crate) fn collect_script(session_id: &str) -> String {
    let dir = outbox_dir_expr(session_id);
    format!(
        "d={dir}; \
         if [ -f \"$d/done\" ]; then echo done; else echo pending; fi; \
         if [ -f \"$d/meta.json\" ]; then base64 < \"$d/meta.json\" | tr -d '\\n'; fi; echo; \
         if [ -f \"$d/note.md\" ]; then base64 < \"$d/note.md\" | tr -d '\\n'; fi; echo; \
         exit 0"
    )
}

/// `meta.json` as the helper writes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutboxMeta {
    pub title: String,
    #[serde(default)]
    pub suggested_next_steps: Option<SuggestedNextSteps>,
    /// The turn id the helper was started with; lets the collector tell this
    /// run's note from an earlier one.
    #[serde(default)]
    pub turn: Option<String>,
    #[serde(default)]
    pub written_at: Option<i64>,
    #[serde(default)]
    pub helper_version: Option<u32>,
}

/// The raw outbox contents as collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutboxSnapshot {
    pub done: bool,
    pub meta: Option<OutboxMeta>,
    pub body: Option<String>,
}

pub(crate) fn parse_collect_output(output: &str) -> Result<OutboxSnapshot, String> {
    use base64::Engine;
    let mut lines = output.lines().map(str::trim);
    let done = match lines.next() {
        Some("done") => true,
        Some("pending") => false,
        other => {
            return Err(format!(
                "unexpected outbox status line {other:?} in {output:?}"
            ))
        }
    };
    let decode = |line: Option<&str>, what: &str| -> Result<Option<Vec<u8>>, String> {
        match line.filter(|l| !l.is_empty()) {
            None => Ok(None),
            Some(encoded) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map(Some)
                .map_err(|e| format!("outbox {what} is not valid base64: {e}")),
        }
    };
    let meta = decode(lines.next(), "meta.json")?
        .map(|bytes| {
            serde_json::from_slice::<OutboxMeta>(&bytes)
                .map_err(|e| format!("outbox meta.json does not parse: {e}"))
        })
        .transpose()?;
    let body = decode(lines.next(), "note.md")?
        .map(|bytes| {
            String::from_utf8(bytes).map_err(|e| format!("outbox note.md is not UTF-8: {e}"))
        })
        .transpose()?;
    Ok(OutboxSnapshot { done, meta, body })
}

/// What the collector found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutboxState {
    /// No `done` marker: the agent never completed a `write_note` call (or the
    /// outbox was cleared and nothing has been written since).
    Pending,
    /// A complete note from a different turn than the one asked about.
    StaleTurn { found: Option<String> },
    /// A complete note for this turn.
    Delivered(NoteSubmission),
}

/// Interpret a snapshot for `expected_turn` (`None` accepts any turn, which is
/// what the dead-owner sweep wants: it has no run of its own).
pub(crate) fn outbox_state(
    snapshot: OutboxSnapshot,
    expected_turn: Option<&str>,
) -> Result<OutboxState, String> {
    if !snapshot.done {
        return Ok(OutboxState::Pending);
    }
    let (Some(meta), Some(body)) = (snapshot.meta, snapshot.body) else {
        return Err("outbox is marked done but meta.json or note.md is missing".to_string());
    };
    if let Some(expected) = expected_turn {
        if meta.turn.as_deref() != Some(expected) {
            return Ok(OutboxState::StaleTurn { found: meta.turn });
        }
    }
    let submission = NoteSubmission::from_params(
        WriteNoteParams {
            title: meta.title,
            body,
            suggested_next_steps: meta.suggested_next_steps,
        },
        NoteSubmissionSource::Outbox,
    )
    .map_err(|e| format!("outbox note is invalid: {e}"))?;
    Ok(OutboxState::Delivered(submission))
}

/// Pull the session's outbox from the workstation and interpret it.
pub(crate) fn collect(
    exec: RemoteExec,
    session_id: &str,
    expected_turn: Option<&str>,
) -> Result<OutboxState, String> {
    validate_session_id(session_id)?;
    let output = exec(&["sh", "-c", &collect_script(session_id)])
        .map_err(|e| format!("failed to read the remote note outbox: {e}"))?;
    outbox_state(parse_collect_output(&output)?, expected_turn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use std::process::{Command, Stdio};

    const HELPER_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/remote/staged-note-mcp.cjs"
    );

    fn node() -> Option<String> {
        let output = Command::new("sh")
            .args(["-c", "command -v node"])
            .output()
            .ok()?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (output.status.success() && !path.is_empty()).then_some(path)
    }

    /// Run `args` locally with `HOME` pointed at `home`, mirroring what `ws
    /// exec` does on the workstation.
    fn local_exec(home: &std::path::Path) -> impl Fn(&[&str]) -> Result<String, String> + '_ {
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

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn helper_checksum_is_sha256_hex_of_the_embedded_source() {
        let sum = helper_checksum();
        assert_eq!(sum.len(), 64);
        assert!(sum.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(HELPER_SOURCE.contains("function handleWriteNote"));
    }

    #[test]
    fn session_ids_are_restricted_to_path_safe_characters() {
        assert!(validate_session_id("0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b").is_ok());
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id("../etc").is_err());
        assert!(validate_session_id("a b").is_err());
        assert!(validate_session_id("x\"; rm -rf /").is_err());
    }

    #[test]
    fn install_probe_parses_home_checksum_and_node() {
        let probe = parse_install_probe("/home/bloxer\nabc123\n/usr/bin/node\n").unwrap();
        assert_eq!(
            probe,
            InstallProbe {
                home: "/home/bloxer".into(),
                installed_checksum: Some("abc123".into()),
                node: Some("/usr/bin/node".into()),
            }
        );
        let fresh = parse_install_probe("/home/bloxer\n\n\n").unwrap();
        assert_eq!(fresh.installed_checksum, None);
        assert_eq!(fresh.node, None);
        assert!(parse_install_probe("").is_err());
        assert!(parse_install_probe("not a path\n").is_err());
    }

    /// Records every command and answers the probe from `probe_output`; any
    /// other command succeeds with empty output.
    fn recording_exec<'a>(
        probe_output: &str,
        log: &'a RefCell<Vec<Vec<String>>>,
    ) -> impl Fn(&[&str]) -> Result<String, String> + 'a {
        let probe_output = probe_output.to_string();
        move |args: &[&str]| {
            log.borrow_mut()
                .push(args.iter().map(|a| a.to_string()).collect());
            if args.len() == 3 && args[2].starts_with("d=\"$HOME/.staged/bin\"") {
                Ok(probe_output.clone())
            } else {
                Ok(String::new())
            }
        }
    }

    #[test]
    fn install_uploads_and_records_the_checksum_when_stale() {
        let log = RefCell::new(Vec::new());
        let exec = recording_exec("/home/bloxer\nstale\n/usr/bin/node\n", &log);
        let helper = install_helper(&exec).unwrap();
        assert_eq!(
            helper,
            RemoteNoteHelper {
                node: "/usr/bin/node".into(),
                helper_path: "/home/bloxer/.staged/bin/staged-note-mcp.cjs".into(),
            }
        );
        let commands = log.borrow();
        assert!(
            commands.len() >= 3,
            "probe, upload chunk(s), checksum record"
        );
        let upload = &commands[1][2];
        assert!(upload.contains("base64 -d"), "{upload}");
        assert!(
            upload.contains("mv -f")
                && upload.contains("/home/bloxer/.staged/bin/staged-note-mcp.cjs'"),
            "upload publishes into place atomically: {upload}"
        );
        let record = &commands.last().unwrap()[2];
        assert!(record.contains(&helper_checksum()), "{record}");
        assert!(record.contains("staged-note-mcp.sha256"), "{record}");
    }

    #[test]
    fn install_skips_the_upload_when_the_checksum_is_current() {
        let log = RefCell::new(Vec::new());
        let probe = format!("/home/bloxer\n{}\n/usr/local/bin/node\n", helper_checksum());
        let exec = recording_exec(&probe, &log);
        let helper = install_helper(&exec).unwrap();
        assert_eq!(helper.node, "/usr/local/bin/node");
        assert_eq!(
            log.borrow().len(),
            1,
            "only the probe ran: {:?}",
            log.borrow()
        );
    }

    #[test]
    fn install_fails_without_node() {
        let log = RefCell::new(Vec::new());
        let exec = recording_exec("/home/bloxer\n\n\n", &log);
        let error = install_helper(&exec).unwrap_err();
        assert!(error.contains("`node`"), "{error}");
        assert_eq!(log.borrow().len(), 1);
    }

    #[test]
    fn stdio_server_runs_the_helper_for_the_session_and_turn() {
        let helper = RemoteNoteHelper {
            node: "/usr/bin/node".into(),
            helper_path: "/home/bloxer/.staged/bin/staged-note-mcp.cjs".into(),
        };
        match stdio_mcp_server(&helper, "sess-1", "turn-1") {
            McpServer::Stdio(server) => {
                assert_eq!(server.name, NOTE_MCP_SERVER_NAME);
                assert_eq!(server.command, PathBuf::from("/usr/bin/node"));
                assert_eq!(
                    server.args,
                    vec![
                        "/home/bloxer/.staged/bin/staged-note-mcp.cjs",
                        "sess-1",
                        "turn-1"
                    ]
                );
                assert!(server.env.is_empty());
            }
            other => panic!("expected a stdio server, got {other:?}"),
        }
    }

    #[test]
    fn collect_output_parses_status_and_base64_files() {
        let meta = r#"{"title":"Plan","suggestedNextSteps":{"suggestedNextCommitStep":"Fix it","suggestedNextNoteStep":null},"turn":"t1","writtenAt":1,"helperVersion":1}"#;
        let output = format!(
            "done\n{}\n{}\n",
            b64(meta.as_bytes()),
            b64("body\n---\nmore".as_bytes())
        );
        let snapshot = parse_collect_output(&output).unwrap();
        assert!(snapshot.done);
        let parsed_meta = snapshot.meta.unwrap();
        assert_eq!(parsed_meta.title, "Plan");
        assert_eq!(parsed_meta.turn.as_deref(), Some("t1"));
        assert_eq!(
            parsed_meta
                .suggested_next_steps
                .unwrap()
                .suggested_next_commit_step
                .as_deref(),
            Some("Fix it")
        );
        assert_eq!(snapshot.body.as_deref(), Some("body\n---\nmore"));

        let pending = parse_collect_output("pending\n\n\n").unwrap();
        assert_eq!(
            pending,
            OutboxSnapshot {
                done: false,
                meta: None,
                body: None
            }
        );
        assert!(parse_collect_output("").is_err());
        assert!(parse_collect_output("done\nnot base64!\n\n").is_err());
    }

    fn done_snapshot(turn: Option<&str>) -> OutboxSnapshot {
        OutboxSnapshot {
            done: true,
            meta: Some(OutboxMeta {
                title: "Title".into(),
                suggested_next_steps: Some(SuggestedNextSteps {
                    suggested_next_commit_step: Some("Commit".into()),
                    suggested_next_note_step: None,
                }),
                turn: turn.map(str::to_string),
                written_at: None,
                helper_version: Some(1),
            }),
            body: Some("Body".into()),
        }
    }

    #[test]
    fn outbox_state_distinguishes_pending_stale_and_delivered() {
        let pending = OutboxSnapshot {
            done: false,
            meta: None,
            body: None,
        };
        assert_eq!(
            outbox_state(pending, Some("t1")).unwrap(),
            OutboxState::Pending
        );

        assert_eq!(
            outbox_state(done_snapshot(Some("t0")), Some("t1")).unwrap(),
            OutboxState::StaleTurn {
                found: Some("t0".into())
            }
        );

        match outbox_state(done_snapshot(Some("t1")), Some("t1")).unwrap() {
            OutboxState::Delivered(submission) => {
                assert_eq!(submission.title, "Title");
                assert_eq!(submission.body, "Body");
                assert_eq!(submission.source, NoteSubmissionSource::Outbox);
                assert_eq!(submission.as_note_content(), "# Title\nBody");
                assert_eq!(
                    submission
                        .suggested_next_steps
                        .unwrap()
                        .suggested_next_commit_step
                        .as_deref(),
                    Some("Commit")
                );
            }
            other => panic!("{other:?}"),
        }

        // The sweep has no turn of its own and takes whatever is complete.
        assert!(matches!(
            outbox_state(done_snapshot(Some("t0")), None).unwrap(),
            OutboxState::Delivered(_)
        ));

        let incomplete = OutboxSnapshot {
            done: true,
            meta: None,
            body: Some("Body".into()),
        };
        assert!(outbox_state(incomplete, None).is_err());
    }

    /// Drive the helper as the agent would: newline-delimited JSON-RPC on
    /// stdin, one response per line on stdout. Returns the parsed responses.
    fn run_helper(
        home: &std::path::Path,
        node: &str,
        requests: &[serde_json::Value],
    ) -> Vec<serde_json::Value> {
        let mut child = Command::new(node)
            .arg(HELPER_PATH)
            .args(["sess-1", "turn-1"])
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn helper");
        {
            let mut stdin = child.stdin.take().unwrap();
            for request in requests {
                writeln!(stdin, "{request}").unwrap();
            }
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "helper exited {:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
            .collect()
    }

    fn property_names(schema: &serde_json::Value) -> Vec<String> {
        let mut names: Vec<String> = schema["properties"]
            .as_object()
            .expect("object schema")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// The nested next-steps object, whether written inline (`properties`) or
    /// as the first branch of an `anyOf`/`oneOf` nullable union.
    fn nested_steps_schema(schema: &serde_json::Value) -> serde_json::Value {
        let steps = &schema["properties"]["suggested_next_steps"];
        if steps.get("properties").is_some() {
            return steps.clone();
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(branches) = steps[key].as_array() {
                if let Some(obj) = branches.iter().find(|b| b.get("properties").is_some()) {
                    return obj.clone();
                }
                // schemars may emit `$ref` branches; resolve against `$defs`.
                for branch in branches {
                    if let Some(reference) = branch["$ref"].as_str() {
                        let name = reference.rsplit('/').next().unwrap();
                        let def = &schema["$defs"][name];
                        if def.get("properties").is_some() {
                            return def.clone();
                        }
                    }
                }
            }
        }
        if let Some(reference) = steps["$ref"].as_str() {
            let name = reference.rsplit('/').next().unwrap();
            return schema["$defs"][name].clone();
        }
        panic!("cannot find the suggested_next_steps object in {schema}");
    }

    #[test]
    fn helper_speaks_mcp_and_writes_the_outbox() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping helper subprocess test");
            return;
        };
        let home = tempfile::tempdir().unwrap();

        // A rejected call on its own must leave no trace in the outbox.
        let rejected = run_helper(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"","body":"x"}}}),
            ],
        );
        assert_eq!(rejected[0]["result"]["isError"], true);
        assert!(
            !home.path().join(".staged/sessions/sess-1").exists(),
            "a rejected call writes nothing"
        );

        let responses = run_helper(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
                serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"","body":"x"}}}),
                serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"# Plan","body":"  Body line\n---\nmore  ","suggested_next_steps":{"suggestedNextCommitStep":" Fix it ","suggestedNextNoteStep":""}}}}),
                serde_json::json!({"jsonrpc":"2.0","id":5,"method":"nope"}),
            ],
        );
        assert_eq!(responses.len(), 5, "{responses:?}");

        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "staged");
        assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");

        let tools = responses[1]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "write_note");
        assert_eq!(
            tools[0]["description"].as_str().unwrap(),
            crate::note_tool::WRITE_NOTE_DESCRIPTION
        );

        assert_eq!(responses[2]["id"], 3);
        assert_eq!(responses[2]["result"]["isError"], true);

        assert_eq!(responses[3]["id"], 4);
        assert_eq!(responses[3]["result"]["isError"], false);
        let text = responses[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert_eq!(
            text,
            crate::note_tool::saved_message("Plan", "Body line\n---\nmore".len())
        );

        assert_eq!(responses[4]["error"]["code"], -32601);

        let outbox = home.path().join(".staged/sessions/sess-1");
        assert!(outbox.join("done").exists());
        assert_eq!(
            std::fs::read_to_string(outbox.join("note.md")).unwrap(),
            "Body line\n---\nmore"
        );
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(outbox.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["title"], "Plan");
        assert_eq!(meta["turn"], "turn-1");
        assert_eq!(
            meta["suggestedNextSteps"]["suggestedNextCommitStep"],
            "Fix it"
        );
        assert_eq!(
            meta["suggestedNextSteps"]["suggestedNextNoteStep"],
            serde_json::Value::Null
        );

        // The collector script against the same outbox, under `sh`.
        let exec = local_exec(home.path());
        match collect(&exec, "sess-1", Some("turn-1")).unwrap() {
            OutboxState::Delivered(submission) => {
                assert_eq!(submission.title, "Plan");
                assert_eq!(submission.body, "Body line\n---\nmore");
                assert_eq!(
                    submission.suggested_next_steps,
                    Some(SuggestedNextSteps {
                        suggested_next_commit_step: Some("Fix it".into()),
                        suggested_next_note_step: None,
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            collect(&exec, "sess-1", Some("turn-2")).unwrap(),
            OutboxState::StaleTurn {
                found: Some("turn-1".into())
            }
        );

        clear_outbox(&exec, "sess-1").unwrap();
        assert_eq!(
            collect(&exec, "sess-1", None).unwrap(),
            OutboxState::Pending
        );
        assert_eq!(
            collect(&exec, "never-ran", None).unwrap(),
            OutboxState::Pending
        );
    }

    #[test]
    fn helper_schema_matches_tool_schema() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping helper schema test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let responses = run_helper(
            home.path(),
            &node,
            &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
        );
        let helper_schema = &responses[0]["result"]["tools"][0]["inputSchema"];
        let tool_schema = crate::note_tool::write_note_input_schema();

        assert_eq!(property_names(helper_schema), property_names(&tool_schema));
        assert_eq!(helper_schema["required"], tool_schema["required"]);
        assert_eq!(
            property_names(&nested_steps_schema(helper_schema)),
            property_names(&nested_steps_schema(&tool_schema)),
            "helper: {helper_schema}\ntool: {tool_schema}"
        );
    }

    #[test]
    fn install_scripts_work_against_a_local_shell() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping local install test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());

        let helper = install_helper(&exec).unwrap();
        assert_eq!(helper.node, node);
        let installed = home.path().join(".staged/bin/staged-note-mcp.cjs");
        assert_eq!(helper.helper_path, installed.to_string_lossy());
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), HELPER_SOURCE);
        assert_eq!(
            std::fs::read_to_string(home.path().join(".staged/bin/staged-note-mcp.sha256"))
                .unwrap(),
            helper_checksum()
        );

        // Second install: the probe sees the current checksum and uploads nothing.
        let probe =
            parse_install_probe(&exec(&["sh", "-c", &install_probe_script()]).unwrap()).unwrap();
        assert_eq!(
            probe.installed_checksum.as_deref(),
            Some(helper_checksum().as_str())
        );
        std::fs::write(&installed, "tampered").unwrap();
        install_helper(&exec).unwrap();
        assert_eq!(
            std::fs::read_to_string(&installed).unwrap(),
            "tampered",
            "a matching checksum file short-circuits the upload"
        );
    }
}
