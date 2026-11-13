//! The app's side of `stagedctl project open`: the body of `dispatch`'s
//! `open_project` arm, reachable through the control server only.
//!
//! [`open_project`] checks the project exists, picks the window the user was
//! last in, parks a [`ProjectOpenRequest`](super::ProjectOpenRequest) for it,
//! tells that window alone with [`PROJECT_OPEN_REQUESTED_EVENT`], raises it,
//! and waits for what the window did. A window that was not listening yet
//! finds the request in its
//! [`list_pending_project_opens`](super::commands::list_pending_project_opens)
//! snapshot instead.
//!
//! **A window still loading is not shown early.** On a cold start the request
//! arrives while the window is hidden, waiting for its frontend to apply the
//! theme and show it. The request is parked for it all the same, but the
//! raise waits for that reveal
//! ([`raise_window_once_revealed`](crate::app_lifecycle::window_raise::raise_window_once_revealed)),
//! which may come after the window has answered.
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
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};

use super::{ProjectOpenAnswer, ProjectOpenOutcome, ProjectOpenState, ANSWER_TIMEOUT};
use crate::app_lifecycle::window_raise::{
    is_on_screen, pick_window_to_show, raise_window_once_revealed,
};
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
    open_project_within(app, store, project_id, ANSWER_TIMEOUT, is_on_screen).await
}

/// [`open_project`] with the wait passed in, and the probe that says whether
/// a window is on screen: the mock runtime calls every window visible, so the
/// tests pass their own to stand in for one that is still loading.
async fn open_project_within<R: Runtime>(
    app: &AppHandle<R>,
    store: &Store,
    project_id: &str,
    timeout: Duration,
    is_on_screen: impl Fn(&WebviewWindow<R>) -> bool + Send + 'static,
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
    // it hears it. A window still loading hears nothing, and finds the
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
    // then showing the user the dialog or comment that is in the way. A
    // window its frontend has not revealed yet is raised when it has been,
    // not now, so this never shows an unpainted webview.
    raise_window_once_revealed(&window, is_on_screen);

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
    use crate::app_lifecycle::UsedWindows;
    use crate::project_open::ProjectOpenRequest;
    use crate::store::Project;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::Listener;

    /// A mock app managing the state and an empty record of used windows,
    /// with one window per label, a store holding one project, and that
    /// project's id.
    fn fixture(labels: &[&str]) -> (tauri::App<MockRuntime>, Store, String) {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(ProjectOpenState::default());
        app.manage(UsedWindows::default());
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

    /// Only the window the request is parked for hears about it. With no
    /// window focused, as when the caller's terminal is frontmost, that is the
    /// window the user was last in, however many others are visible.
    #[tokio::test]
    async fn the_last_used_window_is_told_and_no_other() {
        let (app, store, project_id) = fixture(&["main", "win-2", "win-3"]);
        let heard = [
            heard_by(&app, "main"),
            heard_by(&app, "win-2"),
            heard_by(&app, "win-3"),
        ];
        let state = app.state::<ProjectOpenState>();
        let used = app.state::<UsedWindows>();
        used.note("win-3");
        used.note("win-2");

        // Run more than once: the pick must not vary between calls.
        for round in 1..=5 {
            let (result, ()) =
                tokio::join!(open_project(app.handle(), &store, &project_id), async {
                    assert_eq!(state.pending_for("main"), None);
                    assert_eq!(state.pending_for("win-3"), None);
                    answer(&app, "win-2", ProjectOpenAnswer::Opened);
                });

            assert_eq!(
                result.unwrap(),
                json!({ "outcome": "opened", "window": "win-2" })
            );
            assert!(
                heard[0].lock().unwrap().is_empty(),
                "main must not navigate"
            );
            assert_eq!(heard[1].lock().unwrap().len(), round);
            assert!(
                heard[2].lock().unwrap().is_empty(),
                "win-3 must not navigate"
            );
        }
    }

    /// With no window used yet the pick is still the same every time: the
    /// mock runtime calls every window visible, and `main` leads that tier.
    #[tokio::test]
    async fn with_no_window_used_yet_main_is_told() {
        let (app, store, project_id) = fixture(&["win-3", "main", "win-2"]);

        for _ in 0..5 {
            let (result, ()) =
                tokio::join!(open_project(app.handle(), &store, &project_id), async {
                    answer(&app, "main", ProjectOpenAnswer::Opened)
                });
            assert_eq!(
                result.unwrap(),
                json!({ "outcome": "opened", "window": "main" })
            );
        }
    }

    /// The window is raised as it is asked, and so becomes the last used.
    #[tokio::test]
    async fn a_window_on_screen_is_raised_before_it_answers() {
        let (app, store, project_id) = fixture(&["main"]);
        let used = app.state::<UsedWindows>();

        let (result, ()) = tokio::join!(open_project(app.handle(), &store, &project_id), async {
            assert!(used.contains("main"), "raised before any answer");
            answer(&app, "main", ProjectOpenAnswer::DialogOpen)
        });

        assert_eq!(
            result.unwrap(),
            json!({ "outcome": "dialogOpen", "window": "main" })
        );
    }

    /// The cold start. The window is hidden until its frontend shows it, so
    /// the request is parked and answered as usual, but nothing is raised
    /// until that reveal, which here comes after the answer.
    #[tokio::test(start_paused = true)]
    async fn a_window_still_loading_is_asked_at_once_and_raised_only_when_revealed() {
        let (app, store, project_id) = fixture(&["main"]);
        let heard = heard_by(&app, "main");
        let state = app.state::<ProjectOpenState>();
        let used = app.state::<UsedWindows>();
        let shown = Arc::new(AtomicBool::new(false));
        let is_shown = {
            let shown = Arc::clone(&shown);
            move |_: &WebviewWindow<MockRuntime>| shown.load(Ordering::SeqCst)
        };

        let (result, parked) = tokio::join!(
            open_project_within(app.handle(), &store, &project_id, ANSWER_TIMEOUT, is_shown),
            async {
                // What the loading window's snapshot will offer it.
                let parked = state
                    .pending_for("main")
                    .expect("the request should be parked for the loading window");
                assert!(!used.contains("main"), "not raised while it is loading");
                // The window loads for a while before its listener is live.
                tokio::time::sleep(Duration::from_secs(2)).await;
                assert!(!used.contains("main"), "still not raised");
                assert!(state.answer("main", &parked.request_id, ProjectOpenAnswer::Opened));
                parked
            }
        );

        assert_eq!(
            result.unwrap(),
            json!({ "outcome": "opened", "window": "main" })
        );
        assert_eq!(*heard.lock().unwrap(), vec![as_wire(&parked)]);
        assert!(
            !used.contains("main"),
            "answering is not the reveal: the frontend has not shown the window"
        );

        // The frontend shows its window; the raise that was owed follows.
        shown.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(used.contains("main"), "raised once revealed");
    }

    /// A window that does not answer in time is still owed its raise: the caller reads
    /// the timeout, and the window comes to the front when it does appear.
    #[tokio::test(start_paused = true)]
    async fn a_loading_window_whose_caller_timed_out_is_still_raised_when_revealed() {
        let (app, store, project_id) = fixture(&["main"]);
        let used = app.state::<UsedWindows>();
        let shown = Arc::new(AtomicBool::new(false));
        let is_shown = {
            let shown = Arc::clone(&shown);
            move |_: &WebviewWindow<MockRuntime>| shown.load(Ordering::SeqCst)
        };

        let result =
            open_project_within(app.handle(), &store, &project_id, ANSWER_TIMEOUT, is_shown).await;

        assert_eq!(
            result.unwrap(),
            json!({ "outcome": "timedOut", "window": "main" })
        );
        assert!(!used.contains("main"));

        shown.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(used.contains("main"));
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
