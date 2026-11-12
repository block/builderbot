//! The Staged MCP on Blox: Staged's MCP server on a Blox workstation.
//!
//! A remote agent cannot reach the MCP servers Staged runs on the laptop's
//! loopback, and the laptop may not even be connected when the agent
//! finishes. So remote sessions get Staged's tools from a dependency-free
//! stdio MCP server (`resources/remote/staged-mcp.cjs`, Node standard library
//! only, CommonJS so it runs under any `package.json` module type; Blox images
//! ship `node`) that Staged uploads to the workstation and injects into
//! `session/new` as [`McpServer::Stdio`] under the name [`SERVER_NAME`]. Its
//! tools write what they produce into a per-session scratch directory on the
//! workstation, which the laptop reads over `ws exec` — after the turn, or on
//! the dead-owner sweep when it lost the session — so the output survives a
//! laptop disconnect:
//!
//! ```text
//! ~/.staged/bin/staged-mcp.cjs       the server script (checksum-gated upload)
//! ~/.staged/bin/staged-mcp.sha256    SHA-256 of the installed script
//! ~/.staged/sessions/<session-id>/   the session's scratch directory
//! ```
//!
//! This module owns what every tool shares: installing the script
//! ([`install`]), the stdio injection ([`stdio_mcp_server`]), the remote
//! command runner ([`RemoteExec`]) and the session directory. Each tool's
//! laptop-side reader is a submodule. `write_note`, the first tool, writes the
//! note outbox that [`note_outbox`] collects. See `docs/staged-mcp-on-blox.md`.
//!
//! Everything that talks to the workstation goes through a caller-provided
//! runner (`&dyn Fn(&[&str]) -> Result<String, String>`), the same shape
//! [`crate::session_commands::write_bytes_to_remote_with`] takes, so the
//! command sequences are unit-testable without a workspace and the shell
//! scripts can be exercised locally against `sh`.

pub(crate) mod note_outbox;
#[cfg(test)]
pub(crate) mod test_support;

use std::path::PathBuf;

use acp_client::{McpServer, McpServerStdio};
use sha2::{Digest, Sha256};

use crate::note_tool::NOTE_MCP_SERVER_NAME;

/// The server script, embedded so the running build always installs the
/// version it was built with.
pub(crate) const SCRIPT_SOURCE: &str = include_str!("../resources/remote/staged-mcp.cjs");

const SCRIPT_FILE_NAME: &str = "staged-mcp.cjs";
const SCRIPT_CHECKSUM_FILE_NAME: &str = "staged-mcp.sha256";

/// What builds from before the rename installed in `~/.staged/bin`, when the
/// server was a note-only helper. [`install`] removes them.
const LEGACY_FILE_NAMES: [&str; 2] = ["staged-note-mcp.cjs", "staged-note-mcp.sha256"];

/// The name the server is injected under and answers with in `initialize`.
/// It is also the local `write_note` server's name, so the note tool reads as
/// `mcp__staged__write_note` whichever transport serves it.
pub(crate) const SERVER_NAME: &str = NOTE_MCP_SERVER_NAME;

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

/// SHA-256 (lowercase hex) of the embedded script; the workstation keeps the
/// checksum of what it has installed so an unchanged script is never
/// re-uploaded.
pub(crate) fn script_checksum() -> String {
    let mut hasher = Sha256::new();
    hasher.update(SCRIPT_SOURCE.as_bytes());
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

/// The session's scratch directory as a shell expression (double-quoted
/// `$HOME`), for use inside `sh -c` scripts. The script derives the same path
/// from `os.homedir()`. Callers validate `session_id` first.
pub(crate) fn session_dir_expr(session_id: &str) -> String {
    format!("\"$HOME/.staged/sessions/{session_id}\"")
}

/// Where the Staged MCP on Blox lives on the workstation after [`install`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteStagedMcp {
    /// Absolute path of the workstation's `node`.
    pub node: String,
    /// Absolute path of the installed server script.
    pub script_path: String,
    /// The workstation's home directory, as the probe printed it: what the
    /// session scratch paths spelled out to the agent (`note_draft`) start
    /// with.
    pub home: String,
}

/// What the install probe reports about the workstation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallProbe {
    pub home: String,
    pub installed_checksum: Option<String>,
    pub node: Option<String>,
}

/// One round trip that creates `~/.staged/bin`, removes the legacy note-only
/// helper ([`LEGACY_FILE_NAMES`]) if present, and prints, one per line, the
/// home directory, the checksum of the installed script (empty when absent)
/// and the path of `node` (empty when absent). Always exits 0 so `ws exec`
/// returns the output.
pub(crate) fn install_probe_script() -> String {
    let [legacy_script, legacy_checksum] = LEGACY_FILE_NAMES;
    format!(
        "d=\"$HOME/.staged/bin\"; mkdir -p \"$d\" && chmod 700 \"$HOME/.staged\" \"$d\"; \
         rm -f \"$d/{legacy_script}\" \"$d/{legacy_checksum}\"; \
         printf '%s\\n' \"$HOME\"; \
         printf '%s\\n' \"$(cat \"$d/{SCRIPT_CHECKSUM_FILE_NAME}\" 2>/dev/null || true)\"; \
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

/// Make sure the Staged MCP on Blox is installed on the workstation and
/// return where it is. Uploads the script (base64 over `ws exec`, temp +
/// rename) only when the recorded checksum differs from the embedded
/// script's, then records the new checksum. The probe that decides this also
/// removes the legacy note-only helper. Fails when the workstation has no
/// `node`.
pub(crate) fn install(exec: RemoteExec) -> Result<RemoteStagedMcp, String> {
    let probe = exec(&["sh", "-c", &install_probe_script()])
        .map_err(|e| format!("failed to probe the workstation for the Staged MCP on Blox: {e}"))?;
    let probe = parse_install_probe(&probe)?;
    let Some(node) = probe.node else {
        return Err(
            "the workstation has no `node`, which the Staged MCP on Blox needs".to_string(),
        );
    };
    let bin_dir = format!("{}/.staged/bin", probe.home);
    let script_path = format!("{bin_dir}/{SCRIPT_FILE_NAME}");
    let checksum = script_checksum();

    if probe.installed_checksum.as_deref() != Some(checksum.as_str()) {
        crate::session_commands::write_bytes_to_remote_with(
            exec,
            SCRIPT_SOURCE.as_bytes(),
            &script_path,
        )
        .map_err(|e| format!("failed to upload the Staged MCP on Blox: {e}"))?;
        let record = format!(
            "chmod 700 '{script_path}' && printf '%s' '{checksum}' > '{bin_dir}/{SCRIPT_CHECKSUM_FILE_NAME}'"
        );
        exec(&["sh", "-c", &record])
            .map_err(|e| format!("failed to record the Staged MCP on Blox checksum: {e}"))?;
        log::info!("Installed the Staged MCP on Blox at {script_path} ({checksum})");
    } else {
        log::debug!("The Staged MCP on Blox at {script_path} is current ({checksum})");
    }

    Ok(RemoteStagedMcp {
        node,
        script_path,
        home: probe.home,
    })
}

/// The stdio MCP server entry for `session/new`: the workstation's `node`
/// running the script for this session and turn.
pub(crate) fn stdio_mcp_server(
    server: &RemoteStagedMcp,
    session_id: &str,
    turn_id: &str,
) -> McpServer {
    McpServer::Stdio(
        McpServerStdio::new(SERVER_NAME, PathBuf::from(&server.node)).args(vec![
            server.script_path.clone(),
            session_id.to_string(),
            turn_id.to_string(),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::test_support::{local_exec, node, run_script, run_script_lines};
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn script_checksum_is_sha256_hex_of_the_embedded_source() {
        let sum = script_checksum();
        assert_eq!(sum.len(), 64);
        assert!(sum.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(SCRIPT_SOURCE.starts_with("#!/usr/bin/env node"));
        assert!(SCRIPT_SOURCE.contains("const TOOLS = ["));
        assert_eq!(
            SCRIPT_SOURCE,
            std::fs::read_to_string(super::test_support::SCRIPT_PATH).unwrap(),
            "the embedded script is the one in resources/remote"
        );
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
        let server = install(&exec).unwrap();
        assert_eq!(
            server,
            RemoteStagedMcp {
                node: "/usr/bin/node".into(),
                script_path: "/home/bloxer/.staged/bin/staged-mcp.cjs".into(),
                home: "/home/bloxer".into(),
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
            upload.contains("mv -f") && upload.contains("/home/bloxer/.staged/bin/staged-mcp.cjs'"),
            "upload publishes into place atomically: {upload}"
        );
        let record = &commands.last().unwrap()[2];
        assert!(record.contains(&script_checksum()), "{record}");
        assert!(record.contains("/staged-mcp.sha256'"), "{record}");
    }

    #[test]
    fn install_skips_the_upload_when_the_checksum_is_current() {
        let log = RefCell::new(Vec::new());
        let probe = format!("/home/bloxer\n{}\n/usr/local/bin/node\n", script_checksum());
        let exec = recording_exec(&probe, &log);
        let server = install(&exec).unwrap();
        assert_eq!(server.node, "/usr/local/bin/node");
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
        let error = install(&exec).unwrap_err();
        assert!(error.contains("`node`"), "{error}");
        assert_eq!(log.borrow().len(), 1);
    }

    /// The legacy cleanup rides on the probe, so it costs no extra round trip
    /// and runs even when the current script needs no upload.
    #[test]
    fn install_probe_removes_the_legacy_note_helper() {
        let script = install_probe_script();
        assert!(
            script.contains("rm -f \"$d/staged-note-mcp.cjs\" \"$d/staged-note-mcp.sha256\";"),
            "{script}"
        );
        assert!(
            !script.contains("rm -f \"$d/staged-mcp"),
            "the current script is never removed: {script}"
        );
        let log = RefCell::new(Vec::new());
        let probe = format!("/home/bloxer\n{}\n/usr/bin/node\n", script_checksum());
        install(&recording_exec(&probe, &log)).unwrap();
        assert_eq!(log.borrow().len(), 1, "{:?}", log.borrow());
        assert_eq!(log.borrow()[0][2], script);
    }

    #[test]
    fn stdio_server_runs_the_script_for_the_session_and_turn() {
        let server = RemoteStagedMcp {
            node: "/usr/bin/node".into(),
            script_path: "/home/bloxer/.staged/bin/staged-mcp.cjs".into(),
            home: "/home/bloxer".into(),
        };
        assert_eq!(SERVER_NAME, "staged");
        match stdio_mcp_server(&server, "sess-1", "turn-1") {
            McpServer::Stdio(stdio) => {
                assert_eq!(stdio.name, SERVER_NAME);
                assert_eq!(stdio.command, PathBuf::from("/usr/bin/node"));
                assert_eq!(
                    stdio.args,
                    vec![
                        "/home/bloxer/.staged/bin/staged-mcp.cjs",
                        "sess-1",
                        "turn-1"
                    ]
                );
                assert!(stdio.env.is_empty());
            }
            other => panic!("expected a stdio server, got {other:?}"),
        }
    }

    #[test]
    fn install_scripts_work_against_a_local_shell() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping local install test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());

        let server = install(&exec).unwrap();
        assert_eq!(server.node, node);
        let installed = home.path().join(".staged/bin/staged-mcp.cjs");
        assert_eq!(server.script_path, installed.to_string_lossy());
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), SCRIPT_SOURCE);
        assert_eq!(
            std::fs::read_to_string(home.path().join(".staged/bin/staged-mcp.sha256")).unwrap(),
            script_checksum()
        );

        // Second install: the probe sees the current checksum and uploads nothing.
        let probe =
            parse_install_probe(&exec(&["sh", "-c", &install_probe_script()]).unwrap()).unwrap();
        assert_eq!(
            probe.installed_checksum.as_deref(),
            Some(script_checksum().as_str())
        );
        std::fs::write(&installed, "tampered").unwrap();
        install(&exec).unwrap();
        assert_eq!(
            std::fs::read_to_string(&installed).unwrap(),
            "tampered",
            "a matching checksum file short-circuits the upload"
        );
    }

    #[test]
    fn install_removes_the_legacy_note_helper_against_a_local_shell() {
        if node().is_none() {
            eprintln!("node not available; skipping legacy cleanup test");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".staged/bin");
        let plant_legacy = || {
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("staged-note-mcp.cjs"), "// old helper").unwrap();
            std::fs::write(bin.join("staged-note-mcp.sha256"), "0123").unwrap();
        };
        plant_legacy();
        std::fs::write(bin.join("unrelated"), "keep").unwrap();
        let exec = local_exec(home.path());

        install(&exec).unwrap();
        assert!(!bin.join("staged-note-mcp.cjs").exists());
        assert!(!bin.join("staged-note-mcp.sha256").exists());
        assert!(bin.join("staged-mcp.cjs").exists());
        assert!(bin.join("staged-mcp.sha256").exists());
        assert!(bin.join("unrelated").exists(), "only the legacy pair goes");

        // An older build that reinstalls its helper later is cleaned up on
        // the next install too, even when the current script needs no upload.
        plant_legacy();
        install(&exec).unwrap();
        assert!(!bin.join("staged-note-mcp.cjs").exists());
        assert!(!bin.join("staged-note-mcp.sha256").exists());
        assert_eq!(
            std::fs::read_to_string(bin.join("staged-mcp.cjs")).unwrap(),
            SCRIPT_SOURCE
        );

        // Nothing to remove is not an error.
        install(&exec).unwrap();
    }

    #[test]
    fn script_negotiates_a_protocol_version_it_implements() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping protocol test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let initialize = |id: u32, version: Option<&str>| {
            let mut params =
                serde_json::json!({"capabilities":{},"clientInfo":{"name":"test","version":"0"}});
            if let Some(version) = version {
                params["protocolVersion"] = serde_json::Value::String(version.to_string());
            }
            serde_json::json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":params})
        };
        let responses = run_script(
            home.path(),
            &node,
            &[
                initialize(1, Some("2024-11-05")),
                initialize(2, Some("2025-06-18")),
                initialize(3, Some("2099-01-01")),
                initialize(4, None),
            ],
        );
        // A revision we implement is echoed; anything else gets our newest.
        assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(responses[1]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(responses[2]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(responses[3]["result"]["protocolVersion"], "2025-06-18");
    }

    /// The tool-independent half of the protocol: what any tool added to
    /// `TOOLS` gets for free, and what a client gets for anything else.
    #[test]
    fn script_serves_the_tool_registry_over_mcp() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping MCP plumbing test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let lines = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"prompts/list"}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
            "not json",
            "[1,2]",
            r#"{"jsonrpc":"2.0","id":7,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"nope"}"#,
        ]
        .map(String::from);
        let responses = run_script_lines(home.path(), &node, &lines);
        assert_eq!(responses.len(), 9, "{responses:?}");

        let init = &responses[0]["result"];
        assert_eq!(init["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(init["serverInfo"]["version"], "5");
        assert_eq!(init["capabilities"], serde_json::json!({"tools": {}}));
        // The instructions name the session's draft file by absolute path:
        // the one spelling of it an agent that expands neither `~` nor
        // `$HOME` can use.
        assert_eq!(
            init["instructions"],
            format!(
                "Your note draft file is `{}`. This file is your note: write it, edit it, and \
                 when it is ready call write_note (title and suggested next steps) to publish \
                 it; your assistant text is not saved.",
                home.path()
                    .join(".staged/sessions/sess-1/draft.md")
                    .display()
            )
        );

        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"], serde_json::json!({}));

        let tools = responses[2]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["write_note"], "every registered tool, in order");
        for tool in tools {
            assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        }

        assert_eq!(responses[3]["result"], serde_json::json!({"resources": []}));
        assert_eq!(responses[4]["result"], serde_json::json!({"prompts": []}));
        assert_eq!(responses[5]["id"], 6);
        assert_eq!(responses[5]["error"]["code"], -32602);
        assert_eq!(responses[5]["error"]["message"], "Unknown tool: nope");
        assert_eq!(responses[6]["error"]["code"], -32700);
        assert_eq!(responses[6]["id"], serde_json::Value::Null);
        assert_eq!(responses[7]["error"]["code"], -32600);
        assert_eq!(responses[8]["id"], 8);
        assert_eq!(responses[8]["error"]["code"], -32601);
        assert!(
            !home.path().join(".staged/sessions").exists(),
            "no tool ran, so no session directory was created"
        );
    }

    /// A tool's `call` may return a promise. The server answers when it
    /// settles, a rejection takes the same `failureText` path as a throw, a
    /// `failureText` that itself throws falls back to the default text, a
    /// stray failure outside `callTool` (an unawaited rejection, a throwing
    /// callback) is logged and the server keeps answering, and stdin closing
    /// ends the process even while a tool holds a live interval handle.
    #[test]
    fn script_serves_asynchronous_tools_and_survives_stray_failures() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping async tool test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        // The script exports its registry when required, so a driver can add
        // throwaway tools before starting the server.
        let driver_path = home.path().join("driver.cjs");
        std::fs::write(
            &driver_path,
            format!(
                r#"const server = require({script:?});
const base = {{ description: "fixture", inputSchema: {{ type: "object" }} }};
// `async_ok` settles only when `release` is called, so the order of its
// reply against the requests after it follows from dispatch order alone.
let releaseAsyncOk = null;
server.TOOLS.push(
  Object.assign({{
    name: "async_ok",
    call: function () {{
      return new Promise(function (resolve) {{
        releaseAsyncOk = function () {{ resolve({{ text: "resolved later", isError: false }}); }};
      }});
    }},
  }}, base),
  Object.assign({{
    name: "release",
    call: function () {{
      releaseAsyncOk();
      return {{ text: "released", isError: false }};
    }},
  }}, base),
  Object.assign({{
    name: "sync_throw_bad_failure",
    failureText: function () {{ throw new Error("failureText itself broke"); }},
    call: function () {{ throw new Error("sync boom"); }},
  }}, base),
  Object.assign({{
    name: "hold_handle",
    call: function () {{
      // A live handle that would keep node running after stdin closes.
      setInterval(function () {{}}, 1000);
      return Promise.resolve({{ text: "holding", isError: false }});
    }},
  }}, base),
  Object.assign({{
    name: "async_fail",
    failureText: function (error) {{ return "async_fail could not: " + error.message; }},
    call: function () {{ return Promise.reject(new Error("boom")); }},
  }}, base),
  Object.assign({{
    name: "async_fail_default",
    call: async function () {{ throw new Error("kaboom"); }},
  }}, base),
  Object.assign({{
    name: "async_empty",
    call: async function () {{ return undefined; }},
  }}, base),
  Object.assign({{
    name: "stray",
    call: function () {{
      Promise.reject(new Error("nobody awaited"));
      setImmediate(function () {{ throw new Error("late throw"); }});
      return {{ text: "stray ok", isError: false }};
    }},
  }}, base)
);
server.main(process.argv.slice(3));
"#,
                script = test_support::SCRIPT_PATH
            ),
        )
        .unwrap();
        let call = |id: u32, name: &str| {
            serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":{}}})
                .to_string()
        };
        // All lines go down in one write, so they reach readline as one
        // chunk: the `ping` after the broken `failureText` proves the chunk
        // was not abandoned.
        let lines = [
            call(1, "async_ok"),
            call(2, "async_fail"),
            call(3, "async_fail_default"),
            call(4, "async_empty"),
            call(5, "stray"),
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/list"}"#.to_string(),
            call(7, "sync_throw_bad_failure"),
            r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#.to_string(),
            call(9, "hold_handle"),
            call(10, "release"),
        ];
        let run = test_support::run_node_lines(
            driver_path.to_str().unwrap(),
            &[test_support::SCRIPT_PATH],
            home.path(),
            &node,
            &lines,
        );
        // `run_node_lines` already asserted the exit status and that node
        // exited within its timeout: the stray rejection and the late throw
        // did not take node down, and `hold_handle`'s interval did not keep
        // it alive once stdin closed.
        let by_id = |id: u32| {
            run.responses
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("no reply with id {id}: {:?}", run.responses))
        };
        assert_eq!(run.responses.len(), 10, "{:?}", run.responses);

        assert_eq!(
            by_id(1)["result"],
            serde_json::json!({"content":[{"type":"text","text":"resolved later"}],"isError":false}),
            "a resolved promise is the tool's result"
        );
        assert_eq!(
            by_id(2)["result"],
            serde_json::json!({"content":[{"type":"text","text":"async_fail could not: boom"}],"isError":true}),
            "a rejection goes through failureText"
        );
        assert_eq!(
            by_id(3)["result"],
            serde_json::json!({"content":[{"type":"text","text":"async_fail_default failed: kaboom"}],"isError":true}),
            "a rejection without failureText gets the default text"
        );
        assert_eq!(
            by_id(4)["result"],
            serde_json::json!({"content":[{"type":"text","text":"async_empty failed: returned no { text, isError } result"}],"isError":true}),
            "resolving to nothing is a failure, not an empty content block"
        );
        assert_eq!(by_id(5)["result"]["content"][0]["text"], "stray ok");

        let tools = by_id(6)["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "write_note",
                "async_ok",
                "release",
                "sync_throw_bad_failure",
                "hold_handle",
                "async_fail",
                "async_fail_default",
                "async_empty",
                "stray"
            ],
            "the server still answers after the rejections"
        );
        assert_eq!(
            by_id(7)["result"],
            serde_json::json!({"content":[{"type":"text","text":"sync_throw_bad_failure failed: sync boom"}],"isError":true}),
            "a throwing failureText falls back to the default text instead of escaping"
        );
        assert_eq!(
            by_id(8)["result"],
            serde_json::json!({}),
            "the ping in the same chunk as the broken failureText is still answered"
        );
        assert_eq!(by_id(9)["result"]["content"][0]["text"], "holding");
        assert_eq!(by_id(10)["result"]["content"][0]["text"], "released");
        // The pending call did not block anything after it: its reply came
        // after the list, and after the `release` call that settled it.
        let position = |id: u32| run.responses.iter().position(|r| r["id"] == id).unwrap();
        assert!(position(6) < position(1), "{:?}", run.responses);
        assert!(position(10) < position(1), "{:?}", run.responses);
        assert!(
            run.stderr
                .contains("staged-mcp: sync_throw_bad_failure failed: Error: sync boom"),
            "{}",
            run.stderr
        );
        assert!(
            run.stderr.contains(
                "staged-mcp: sync_throw_bad_failure failed while describing its failure: Error: failureText itself broke"
            ),
            "{}",
            run.stderr
        );
        assert!(
            !run.stderr.contains("pending"),
            "every call had settled when stdin closed: {}",
            run.stderr
        );

        assert!(
            run.stderr
                .contains("staged-mcp: async_fail failed: Error: boom"),
            "{}",
            run.stderr
        );
        assert!(
            run.stderr.contains(
                "staged-mcp: unhandled rejection, server kept alive: Error: nobody awaited"
            ),
            "{}",
            run.stderr
        );
        assert!(
            run.stderr
                .contains("staged-mcp: uncaught exception, server kept alive: Error: late throw"),
            "{}",
            run.stderr
        );
    }

    /// A throw out of the request handler itself (here a tool whose
    /// `description` getter throws during `tools/list`) answers `-32603` for
    /// that request and leaves the rest of the stdin chunk served.
    #[test]
    fn script_answers_internal_error_when_a_request_handler_throws() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping request guard test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let driver_path = home.path().join("driver.cjs");
        std::fs::write(
            &driver_path,
            format!(
                r#"const server = require({script:?});
server.TOOLS.push({{
  name: "bad_description",
  get description() {{ throw new Error("description exploded"); }},
  inputSchema: {{ type: "object" }},
  call: function () {{ return {{ text: "never listed", isError: false }}; }},
}});
server.main(process.argv.slice(3));
"#,
                script = test_support::SCRIPT_PATH
            ),
        )
        .unwrap();
        let lines = [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.to_string(),
            r#"{"jsonrpc":"2.0","id":"second","method":"ping"}"#.to_string(),
        ];
        let run = test_support::run_node_lines(
            driver_path.to_str().unwrap(),
            &[test_support::SCRIPT_PATH],
            home.path(),
            &node,
            &lines,
        );
        assert_eq!(
            run.responses,
            vec![
                serde_json::json!({"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"Internal error: description exploded"}}),
                serde_json::json!({"jsonrpc":"2.0","id":"second","result":{}}),
            ],
            "{}",
            run.stderr
        );
        assert!(
            run.stderr
                .contains("staged-mcp: request failed: Error: description exploded"),
            "{}",
            run.stderr
        );
    }
}
