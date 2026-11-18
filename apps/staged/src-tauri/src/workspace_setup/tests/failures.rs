//! What the reason store announces. The reasons are process-wide, so this
//! is the one test that records any.

use tauri::Listener;

use super::*;
use crate::workspace_setup::failures;

/// A reason is announced when it changes, again only for a setup the user
/// asked for, and dropped once its branch is out of `Error`.
#[test]
fn a_reason_is_announced_once_and_dropped_when_its_branch_moves_on() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("failed to build mock app");
    let (heard_tx, heard) = std::sync::mpsc::channel();
    app.listen(failures::EVENT, move |event| {
        let _ = heard_tx.send(event.payload().to_string());
    });
    let heard = || heard.try_iter().collect::<Vec<String>>();
    let (store, project) = store_with_project();
    let [failed, moved_on] = ["feature-a", "feature-b"].map(|name| {
        let branch = starting_branch(&store, &project, name);
        set_status(&store, &branch, WorkspaceStatus::Error)
    });
    let set = |branch: &store::Branch, error, user_asked| {
        failures::set(app.handle(), &store, &branch.id, error, user_asked)
    };
    let event = |branch: &store::Branch, error: &str, user_requested: bool| {
        let id = &branch.id;
        format!(r#"{{"branchId":"{id}","error":{error},"userRequested":{user_requested}}}"#)
    };

    set(&failed, Some("no network"), false);
    set(&moved_on, Some("no network"), false);
    let first = [&failed, &moved_on].map(|b| event(b, r#""no network""#, false));
    assert_eq!(heard(), first);

    // An automatic retry failing the same way says nothing; the user's does.
    set(&failed, Some("no network"), false);
    assert_eq!(heard(), Vec::<String>::new());
    set(&failed, Some("no network"), true);
    assert_eq!(heard(), [event(&failed, r#""no network""#, true)]);

    // A poll moved the other branch on: the next failure drops its reason.
    set_status(&store, &moved_on, WorkspaceStatus::Running);
    set(&failed, Some("no route"), false);
    let pruned = [
        event(&failed, r#""no route""#, false),
        event(&moved_on, "null", false),
    ];
    assert_eq!(heard(), pruned);
    assert_eq!(failures::all().get(&moved_on.id), None);

    // Clearing a reason is announced once.
    set(&failed, None, true);
    set(&failed, None, false);
    assert_eq!(heard(), [event(&failed, "null", false)]);
}
