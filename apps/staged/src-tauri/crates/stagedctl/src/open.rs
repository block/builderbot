//! `stagedctl project open`: ask a Staged window to show a project, and
//! word what the window did.
//!
//! The app's `open_project` command (`src-tauri/src/project_open/control.rs`)
//! picks the window a Dock click would raise, raises it, asks it to switch,
//! and answers `{ "outcome", "window" }` whatever the window did. A window
//! that refused is a result on the wire, not an error, because the control
//! server's plain errors carry no code. This module is where each outcome
//! becomes an exit code and a `code: message` line.
//!
//! The project is resolved here, before the call, so a name that matches
//! nothing never raises a window.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{Client, CtlError};
use crate::commands::{parse, resolve_project, CmdOutput, Project};

/// The dispatch command behind the verb. A Staged from before it answers
/// 404, as for any unknown command, and the transport appends an "update
/// Staged" hint to exactly this name.
pub const OPEN_PROJECT_COMMAND: &str = "open_project";

/// What the window did with the request, as `open_project` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Outcome {
    /// The project is on screen: switched to, or already showing.
    Opened,
    /// A dialog is open in the window; switching would discard it.
    DialogOpen,
    /// The diff comment being written could not be saved first.
    DiffCommentUnsaved,
    /// The window does not have the project (a delete is in flight).
    ProjectNotFound,
    /// A newer request for the same window replaced this one.
    Superseded,
    /// The window did not answer within the app's wait.
    TimedOut,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenAnswer {
    outcome: Outcome,
    /// The label of the window that was asked and raised.
    window: String,
}

/// `project open <project>`: resolve the name or id, then ask the app.
pub fn project_open(client: &Client, needle: &str) -> Result<CmdOutput, CtlError> {
    let project = resolve_project(client, needle)?;
    open_resolved(&project, client.pid, |args| {
        client.invoke(OPEN_PROJECT_COMMAND, args)
    })
}

/// Send the request for an already-resolved project and map the window's
/// answer. `invoke` is the `open_project` call, passed in so the mapping is
/// testable without an app; `pid` is the instance being asked.
fn open_resolved(
    project: &Project,
    pid: u32,
    invoke: impl FnOnce(Value) -> Result<Value, CtlError>,
) -> Result<CmdOutput, CtlError> {
    // Only the id crosses the socket; the app looks the project up itself.
    let raw = invoke(json!({ "projectId": project.id }))?;
    let answer: OpenAnswer = parse(&raw, "open_project answer")?;
    let name = &project.name;
    match answer.outcome {
        Outcome::Opened => Ok(CmdOutput {
            json: json!({
                "outcome": "opened",
                "project": { "id": project.id, "name": name },
                "window": answer.window,
                "pid": pid,
            }),
            human: format!("opened {name}  id:{}  pid:{pid}", project.id),
        }),
        Outcome::DialogOpen => Err(CtlError::command(
            "ui_busy",
            "Staged has a dialog open; close it and run the command again",
        )),
        Outcome::DiffCommentUnsaved => Err(CtlError::command(
            "ui_busy",
            "Staged could not save the diff comment being written; save or discard it in the \
             window and run the command again",
        )),
        // Not a dialog in the way, but the same class of failure: the
        // window was taken up with something else, so nothing was shown
        // for this caller, and running the command again is the remedy.
        Outcome::Superseded => Err(CtlError::command(
            "ui_busy",
            format!(
                "another `stagedctl project open` reached the same Staged window and replaced \
                 this one before the window acted on it, so {name:?} was not shown; run the \
                 command again if it still should be"
            ),
        )),
        Outcome::ProjectNotFound => Err(CtlError::command(
            "not_found",
            format!(
                "the Staged window no longer has project {name:?} (id {}); it was probably \
                 just deleted (see `stagedctl project list`)",
                project.id
            ),
        )),
        Outcome::TimedOut => Err(CtlError::command(
            "ui_timeout",
            "the Staged window was brought to the front but did not answer in time, so the \
             project it shows may not have changed; run the command again",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widgets() -> Project {
        Project {
            id: "p1".to_string(),
            name: "Widgets".to_string(),
            github_repo: None,
            subpath: None,
            location: None,
        }
    }

    /// `open_resolved` against an app that answers `outcome` from `main`.
    fn open_with(outcome: &str) -> Result<CmdOutput, CtlError> {
        let answer = json!({ "outcome": outcome, "window": "main" });
        open_resolved(&widgets(), 4242, move |_| Ok(answer))
    }

    /// The exit code and stderr line of a refused open.
    fn refusal(outcome: &str) -> (i32, String) {
        let err = open_with(outcome).expect_err("the outcome is a failure");
        (err.exit_code(), err.to_string())
    }

    #[test]
    fn the_request_carries_the_project_id_and_nothing_else() {
        let mut sent = None;
        open_resolved(&widgets(), 4242, |args| {
            sent = Some(args);
            Ok(json!({ "outcome": "opened", "window": "main" }))
        })
        .unwrap();
        assert_eq!(sent, Some(json!({ "projectId": "p1" })));
    }

    #[test]
    fn opened_prints_the_name_the_id_and_the_instance_pid() {
        let output = open_with("opened").unwrap();
        assert_eq!(output.human, "opened Widgets  id:p1  pid:4242");
        assert_eq!(
            output.json,
            json!({
                "outcome": "opened",
                "project": { "id": "p1", "name": "Widgets" },
                "window": "main",
                "pid": 4242,
            })
        );
    }

    #[test]
    fn a_dialog_or_an_unsaved_diff_comment_is_ui_busy() {
        assert_eq!(
            refusal("dialogOpen"),
            (
                1,
                "ui_busy: Staged has a dialog open; close it and run the command again".to_string()
            )
        );
        assert_eq!(
            refusal("diffCommentUnsaved"),
            (
                1,
                "ui_busy: Staged could not save the diff comment being written; save or \
                 discard it in the window and run the command again"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_superseded_request_is_ui_busy_with_its_own_message() {
        let (exit, line) = refusal("superseded");
        assert_eq!(exit, 1);
        assert_eq!(
            line,
            "ui_busy: another `stagedctl project open` reached the same Staged window and \
             replaced this one before the window acted on it, so \"Widgets\" was not shown; \
             run the command again if it still should be"
        );
        assert_ne!(line, refusal("dialogOpen").1);
    }

    #[test]
    fn a_project_the_window_lost_is_not_found() {
        assert_eq!(
            refusal("projectNotFound"),
            (
                1,
                "not_found: the Staged window no longer has project \"Widgets\" (id p1); it \
                 was probably just deleted (see `stagedctl project list`)"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_window_that_never_answered_is_ui_timeout() {
        assert_eq!(
            refusal("timedOut"),
            (
                1,
                "ui_timeout: the Staged window was brought to the front but did not answer \
                 in time, so the project it shows may not have changed; run the command again"
                    .to_string()
            )
        );
    }

    #[test]
    fn every_outcome_the_app_names_is_understood() {
        // The names `project_open::control::outcome_name` returns app-side.
        for (name, outcome) in [
            ("opened", Outcome::Opened),
            ("dialogOpen", Outcome::DialogOpen),
            ("diffCommentUnsaved", Outcome::DiffCommentUnsaved),
            ("projectNotFound", Outcome::ProjectNotFound),
            ("superseded", Outcome::Superseded),
            ("timedOut", Outcome::TimedOut),
        ] {
            let answer: OpenAnswer =
                serde_json::from_value(json!({ "outcome": name, "window": "w2" })).unwrap();
            assert_eq!(answer.outcome, outcome, "{name}");
            assert_eq!(answer.window, "w2");
        }
    }

    #[test]
    fn an_outcome_this_cli_does_not_know_is_drift_not_success() {
        let err = open_with("minimized").unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.to_string()
                .contains("unexpected open_project answer shape"),
            "{err}"
        );
        // Nor is an answer without an outcome.
        let err = open_resolved(&widgets(), 4242, |_| Ok(json!({ "window": "main" }))).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn the_apps_own_refusals_pass_through_untouched() {
        // Scope and credential refusals are exit 4 with the app's code.
        let err = open_resolved(&widgets(), 4242, |_| {
            Err(CtlError::Authorization {
                code: "scope_denied".to_string(),
                message: "open_project changes the app and this credential is read-only".into(),
            })
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 4);
        assert_eq!(
            err.to_string(),
            "scope_denied: open_project changes the app and this credential is read-only"
        );
        // A request the app could not make at all stays the app's error.
        let err = open_resolved(&widgets(), 4242, |_| {
            Err(CtlError::command(
                "app_error",
                "Staged has no window to show the project in; restart Staged and try again",
            ))
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(err.code(), "app_error");
    }
}
