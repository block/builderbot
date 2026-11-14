//! Tauri commands a window uses to take and answer its project-open request.
//! Both act on the calling window only, by its label, so one window can
//! neither see nor answer another's request. Desktop only:
//! `web_server::dispatch` refuses them, and they stay out of
//! `control_server::ALLOWED_COMMANDS`, so no stagedctl caller can answer the
//! request it made.
//!
//! Generic over the runtime so the tests below can call them with windows
//! from tauri's mock runtime; `generate_handler!` instantiates them with
//! `Wry`.

use tauri::{Runtime, State, WebviewWindow};

use super::{ProjectOpenAnswer, ProjectOpenRequest, ProjectOpenState};

/// The request this window has yet to answer: none, or one. The frontend
/// takes this snapshot once its event listener is live, so a request that
/// arrived before the window loaded is still acted on. It reads it again
/// right before it navigates for a request, and leaves the window alone when
/// that request is no longer listed.
#[tauri::command]
pub fn list_pending_project_opens<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, ProjectOpenState>,
) -> Vec<ProjectOpenRequest> {
    state.pending_for(window.label()).into_iter().collect()
}

/// Report what this window did with request `request_id`. Fails when the
/// request is no longer this window's to answer — it was superseded, or its
/// caller stopped waiting — so the frontend can tell its answer went nowhere.
#[tauri::command]
pub fn answer_project_open<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, ProjectOpenState>,
    request_id: String,
    outcome: ProjectOpenAnswer,
) -> Result<(), String> {
    if state.answer(window.label(), &request_id, outcome) {
        Ok(())
    } else {
        Err("that project-open request is no longer pending".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_open::{ProjectOpenOutcome, ANSWER_TIMEOUT};
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::Manager;

    /// A mock app managing the state, with one window per label.
    fn mock_app(labels: &[&str]) -> tauri::App<MockRuntime> {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("failed to build mock app");
        app.manage(ProjectOpenState::default());
        for label in labels {
            tauri::WebviewWindowBuilder::new(&app, *label, Default::default())
                .build()
                .expect("failed to create mock window");
        }
        app
    }

    fn window(app: &tauri::App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
        app.get_webview_window(label).expect("window should exist")
    }

    #[test]
    fn a_window_lists_only_its_own_request() {
        let app = mock_app(&["main", "win-2"]);
        let state = app.state::<ProjectOpenState>();
        let pending = state.request("win-2", "p1");

        assert_eq!(
            list_pending_project_opens(window(&app, "win-2"), app.state()),
            vec![pending.request().clone()]
        );
        assert!(list_pending_project_opens(window(&app, "main"), app.state()).is_empty());
    }

    #[tokio::test]
    async fn a_window_answers_only_its_own_request() {
        let app = mock_app(&["main", "win-2"]);
        let state = app.state::<ProjectOpenState>();
        let pending = state.request("win-2", "p1");
        let id = pending.request().request_id.clone();

        let refused = answer_project_open(
            window(&app, "main"),
            app.state(),
            id.clone(),
            ProjectOpenAnswer::Opened,
        );
        assert_eq!(
            refused.unwrap_err(),
            "that project-open request is no longer pending"
        );

        answer_project_open(
            window(&app, "win-2"),
            app.state(),
            id.clone(),
            ProjectOpenAnswer::DiffCommentUnsaved,
        )
        .expect("the request's own window answers it");
        assert_eq!(
            pending.wait(ANSWER_TIMEOUT).await,
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::DiffCommentUnsaved)
        );

        // Answered once: the request is gone, for its own window too.
        assert!(list_pending_project_opens(window(&app, "win-2"), app.state()).is_empty());
        assert!(answer_project_open(
            window(&app, "win-2"),
            app.state(),
            id,
            ProjectOpenAnswer::Opened,
        )
        .is_err());
    }
}
