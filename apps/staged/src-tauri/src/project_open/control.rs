//! The app's side of `stagedctl project open`: the body of `dispatch`'s
//! `open_project` arm, reachable through the control server only.
//!
//! [`open_project`] checks the project exists, picks the window a Dock click
//! would land on, parks a [`ProjectOpenRequest`](super::ProjectOpenRequest)
//! for it, tells that window alone with [`PROJECT_OPEN_REQUESTED_EVENT`],
//! raises it, and waits for what the window did. A window that was not
//! listening yet finds the request in its
//! [`list_pending_project_opens`](super::commands::list_pending_project_opens)
//! snapshot instead.
//!
//! **A refusal is a result, not an error.** The answer goes back as
//! `{ "outcome", "window" }` whatever the window did, so the CLI can tell a
//! dialog in the way from a window that never answered and word each for the
//! user. Only a request that could not be made at all — an unknown project,
//! no window left — is an `Err`.
//!
//! Generic over the runtime so the tests below can drive it with windows
//! from tauri's mock runtime; `dispatch` calls it with `Wry`.

use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::{ProjectOpenAnswer, ProjectOpenOutcome, ProjectOpenState, ANSWER_TIMEOUT};
use crate::app_lifecycle::window_raise::{pick_window_to_show, raise_window};
use crate::store::Store;

/// Emitted to one window, by label, with the
/// [`ProjectOpenRequest`](super::ProjectOpenRequest) it is asked to act on.
pub const PROJECT_OPEN_REQUESTED_EVENT: &str = "project-open-requested";

/// Ask a window to show `project_id` and report what it did, waiting up to
/// [`ANSWER_TIMEOUT`].
pub(crate) async fn open_project<R: Runtime>(
    app: &AppHandle<R>,
    store: &Store,
    project_id: &str,
) -> Result<Value, String> {
    open_project_within(app, store, project_id, ANSWER_TIMEOUT).await
}

/// [`open_project`] with the wait passed in.
async fn open_project_within<R: Runtime>(
    app: &AppHandle<R>,
    store: &Store,
    project_id: &str,
    timeout: Duration,
) -> Result<Value, String> {
    // Before any window is touched: a typo must not pull Staged to the front.
    store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let state = app
        .try_state::<ProjectOpenState>()
        .ok_or("open_project is not available: its request state is not managed")?;
    let window = pick_window_to_show(app)
        .ok_or("Staged has no window to show the project in; restart Staged and try again")?;
    let label = window.label().to_string();

    // Parked before the event goes out, so the window can answer the moment
    // it hears it, and before the raise, so a window still loading finds the
    // request in its snapshot.
    let pending = state.request(&label, project_id);
    if let Err(e) = app.emit_to(
        label.as_str(),
        PROJECT_OPEN_REQUESTED_EVENT,
        pending.request(),
    ) {
        // Not fatal: the snapshot still offers the request.
        log::warn!("Failed to emit {PROJECT_OPEN_REQUESTED_EVENT} to {label}: {e}");
    }
    // Raised whatever the answer turns out to be: a window that refuses is
    // then showing the user the dialog or comment that is in the way.
    raise_window(&window);

    let outcome = pending.wait(timeout).await;
    Ok(json!({ "outcome": outcome_name(outcome), "window": label }))
}

/// The `outcome` a control caller reads. The answers carry the names the
/// window gave them on the way in.
fn outcome_name(outcome: ProjectOpenOutcome) -> &'static str {
    match outcome {
        ProjectOpenOutcome::Answered(ProjectOpenAnswer::Opened) => "opened",
        ProjectOpenOutcome::Answered(ProjectOpenAnswer::DialogOpen) => "dialogOpen",
        ProjectOpenOutcome::Answered(ProjectOpenAnswer::DiffCommentUnsaved) => "diffCommentUnsaved",
        ProjectOpenOutcome::Answered(ProjectOpenAnswer::ProjectNotFound) => "projectNotFound",
        ProjectOpenOutcome::Superseded => "superseded",
        ProjectOpenOutcome::TimedOut => "timedOut",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_open::ProjectOpenRequest;
    use crate::store::Project;
    use std::sync::{Arc, Mutex};
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::Listener;

    /// A mock app managing the state with one window per label, a store
    /// holding one project, and that project's id.
    fn fixture(labels: &[&str]) -> (tauri::App<MockRuntime>, Store, String) {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(ProjectOpenState::default());
        for label in labels {
            tauri::WebviewWindowBuilder::new(&app, *label, Default::default())
                .build()
                .expect("failed to create mock window");
        }
        let store = Store::in_memory().unwrap();
        let project = Project::named("alpha");
        store.create_project(&project).unwrap();
        (app, store, project.id)
    }

    /// The requests `label`'s window hears, in order.
    fn heard_by(app: &tauri::App<MockRuntime>, label: &str) -> Arc<Mutex<Vec<Value>>> {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&heard);
        app.get_webview_window(label)
            .expect("window should exist")
            .listen(PROJECT_OPEN_REQUESTED_EVENT, move |event| {
                sink.lock()
                    .unwrap()
                    .push(serde_json::from_str(event.payload()).unwrap());
            });
        heard
    }

    /// Answer the request parked for `label`, as its window would.
    fn answer(app: &tauri::App<MockRuntime>, label: &str, answer: ProjectOpenAnswer) {
        let state = app.state::<ProjectOpenState>();
        let request = state
            .pending_for(label)
            .expect("a request should be parked");
        assert!(state.answer(label, &request.request_id, answer));
    }

    fn as_wire(request: &ProjectOpenRequest) -> Value {
        serde_json::to_value(request).unwrap()
    }

    #[tokio::test]
    async fn the_picked_window_is_told_and_its_answer_comes_back() {
        let (app, store, project_id) = fixture(&["main"]);
        let heard = heard_by(&app, "main");
        let state = app.state::<ProjectOpenState>();

        // `join!` polls in order: the open runs up to its wait, with the
        // request parked and the event out, before the window's side runs.
        let (result, parked) =
            tokio::join!(open_project(app.handle(), &store, &project_id), async {
                let parked = state
                    .pending_for("main")
                    .expect("a request should be parked");
                assert!(state.answer("main", &parked.request_id, ProjectOpenAnswer::Opened));
                parked
            });

        assert_eq!(
            result.unwrap(),
            json!({ "outcome": "opened", "window": "main" })
        );
        assert_eq!(parked.project_id, project_id);
        assert_eq!(
            *heard.lock().unwrap(),
            vec![as_wire(&parked)],
            "the window hears the request it will find in its snapshot"
        );
        assert_eq!(state.pending_for("main"), None);
    }

    /// A window that refuses is not an error: the caller reads which refusal.
    #[tokio::test]
    async fn a_refusal_comes_back_as_its_outcome() {
        let (app, store, project_id) = fixture(&["main"]);
        for (given, outcome) in [
            (ProjectOpenAnswer::DialogOpen, "dialogOpen"),
            (ProjectOpenAnswer::DiffCommentUnsaved, "diffCommentUnsaved"),
            (ProjectOpenAnswer::ProjectNotFound, "projectNotFound"),
        ] {
            let (result, ()) =
                tokio::join!(open_project(app.handle(), &store, &project_id), async {
                    answer(&app, "main", given)
                });
            assert_eq!(
                result.unwrap(),
                json!({ "outcome": outcome, "window": "main" })
            );
        }
    }

    /// Only the window the request is parked for hears about it.
    #[tokio::test]
    async fn no_other_window_is_told() {
        let (app, store, project_id) = fixture(&["main", "win-2"]);
        let heard = [heard_by(&app, "main"), heard_by(&app, "win-2")];
        let state = app.state::<ProjectOpenState>();

        let (result, picked) =
            tokio::join!(open_project(app.handle(), &store, &project_id), async {
                // The mock runtime calls every window visible, so either may be
                // picked; exactly one holds the request.
                let picked: Vec<&str> = ["main", "win-2"]
                    .into_iter()
                    .filter(|label| state.pending_for(label).is_some())
                    .collect();
                assert_eq!(picked.len(), 1, "one window holds the request: {picked:?}");
                answer(&app, picked[0], ProjectOpenAnswer::Opened);
                picked[0]
            });

        assert_eq!(
            result.unwrap(),
            json!({ "outcome": "opened", "window": picked })
        );
        let (told, untold) = if picked == "main" { (0, 1) } else { (1, 0) };
        assert_eq!(heard[told].lock().unwrap().len(), 1);
        assert!(
            heard[untold].lock().unwrap().is_empty(),
            "the other window must not navigate"
        );
    }

    #[tokio::test]
    async fn an_unknown_project_is_refused_before_any_window_is_told() {
        let (app, store, _) = fixture(&["main"]);
        let heard = heard_by(&app, "main");

        let err = open_project(app.handle(), &store, "nope")
            .await
            .unwrap_err();

        assert_eq!(err, "Project not found: nope");
        assert!(heard.lock().unwrap().is_empty());
        assert_eq!(app.state::<ProjectOpenState>().pending_for("main"), None);
    }

    #[tokio::test]
    async fn with_no_window_left_the_request_fails_without_waiting() {
        let (app, store, project_id) = fixture(&[]);

        let err = open_project(app.handle(), &store, &project_id)
            .await
            .unwrap_err();

        assert!(err.contains("no window"), "got: {err}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_window_that_never_answers_times_out_and_loses_the_request() {
        let (app, store, project_id) = fixture(&["main"]);
        let started = tokio::time::Instant::now();

        let result = open_project(app.handle(), &store, &project_id).await;

        assert_eq!(
            result.unwrap(),
            json!({ "outcome": "timedOut", "window": "main" })
        );
        assert_eq!(started.elapsed(), ANSWER_TIMEOUT);
        assert_eq!(
            app.state::<ProjectOpenState>().pending_for("main"),
            None,
            "a window loading later is not offered the request"
        );
    }

    /// A second call for the same window replaces the first, whose caller is
    /// told so rather than left to time out.
    #[tokio::test]
    async fn a_newer_call_supersedes_the_one_still_waiting() {
        let (app, store, project_id) = fixture(&["main"]);
        let other = Project::named("beta");
        store.create_project(&other).unwrap();
        let heard = heard_by(&app, "main");

        let (older, newer, ()) = tokio::join!(
            open_project(app.handle(), &store, &project_id),
            open_project(app.handle(), &store, &other.id),
            async { answer(&app, "main", ProjectOpenAnswer::Opened) }
        );

        assert_eq!(
            older.unwrap(),
            json!({ "outcome": "superseded", "window": "main" })
        );
        assert_eq!(
            newer.unwrap(),
            json!({ "outcome": "opened", "window": "main" })
        );
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 2);
        assert_eq!(
            heard[1]["projectId"],
            other.id.as_str(),
            "the newer request is the one the window answered"
        );
    }

    #[test]
    fn every_outcome_has_its_own_name() {
        let outcomes = [
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::Opened),
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::DialogOpen),
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::DiffCommentUnsaved),
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::ProjectNotFound),
            ProjectOpenOutcome::Superseded,
            ProjectOpenOutcome::TimedOut,
        ];
        let names: std::collections::BTreeSet<&str> =
            outcomes.into_iter().map(outcome_name).collect();
        assert_eq!(names.len(), outcomes.len());

        // An answer goes out under the name the window sent it in as.
        for answer in [
            ProjectOpenAnswer::Opened,
            ProjectOpenAnswer::DialogOpen,
            ProjectOpenAnswer::DiffCommentUnsaved,
            ProjectOpenAnswer::ProjectNotFound,
        ] {
            let name = outcome_name(ProjectOpenOutcome::Answered(answer));
            assert_eq!(
                serde_json::from_value::<ProjectOpenAnswer>(json!(name)).unwrap(),
                answer
            );
        }
    }
}
