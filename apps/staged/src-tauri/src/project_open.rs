//! Requests to show a project that come from outside the window, held until
//! that window answers.
//!
//! `stagedctl project open` asks a running app to put a project on screen.
//! The backend cannot do that on its own: every window has its own selection,
//! and only the window knows whether a dialog is open or a diff comment still
//! needs saving. So the caller ([`control::open_project`], the control
//! server's `open_project` command) parks a [`ProjectOpenRequest`] here for
//! the window it picked, and that window reports what it did through
//! [`answer_project_open`](commands::answer_project_open).
//!
//! **Not lost before the window loads.** A request can arrive before the
//! window's event listener is live — the CLI may have only just launched the
//! app. As with the consent dialog (`control_server::consent`), the frontend
//! pairs the event with a snapshot: once it is listening it takes
//! [`list_pending_project_opens`](commands::list_pending_project_opens).
//!
//! **One per window.** A newer request for a window supersedes the older one,
//! whose caller reads [`ProjectOpenOutcome::Superseded`], so requests cannot
//! queue up navigations behind each other.
//!
//! **Only while someone waits.** A request lives exactly as long as the
//! [`PendingProjectOpen`] its caller holds. When the wait times out, or the
//! caller goes away without waiting it out, the request is withdrawn, so
//! there is nothing to expire or prune. From then on the snapshot no longer
//! lists it and an answer to it is refused. The same holds for a request a
//! newer one superseded.
//!
//! **What a failed caller can rely on.** This state decides what the caller
//! is told. It cannot stop a window that has already taken the request. The
//! window (`projectOpenListener.ts`) therefore reads the snapshot again right
//! before it navigates, after any wait on its project list, on a diff comment
//! being saved, or behind an earlier request. It navigates only if the
//! request is still listed. So:
//!
//! - A window that comes up after a request was withdrawn or superseded is
//!   never offered it.
//! - A window that had taken the request, and was waiting when that
//!   happened, does not navigate for it.
//! - One gap remains. The timeout or the newer request can land between the
//!   snapshot's reply and the navigation it allows, which is one round trip
//!   to the window. The window then switches project and its answer is
//!   refused. Closing the gap would mean holding this lock across the
//!   window's navigation, which the backend cannot do.
//!
//! [`ProjectOpenOutcome::TimedOut`] and [`ProjectOpenOutcome::Superseded`]
//! therefore mean the window did not report showing the project. They do not
//! promise that it is not on screen.
//!
//! In memory only: nothing here is persisted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

pub mod commands;
pub(crate) mod control;

/// How long a caller waits for the window's answer. A window that is already
/// up answers at once; this covers one that is still loading because the app
/// was launched for the request.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

/// What a window is asked to do: the `list_pending_project_opens` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOpenRequest {
    pub request_id: String,
    pub project_id: String,
}

/// What the window did with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectOpenAnswer {
    /// The project is on screen: the window switched to it, or was already
    /// showing it.
    Opened,
    /// A dialog is open in the window; the project is unchanged.
    DialogOpen,
    /// The diff view could not save the comment being typed, so the window
    /// stayed on it rather than lose the text.
    DiffCommentUnsaved,
    /// The window knows no project with that id.
    ProjectNotFound,
}

/// How a request ended, as its caller reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectOpenOutcome {
    Answered(ProjectOpenAnswer),
    /// A newer request for the same window replaced this one before the
    /// window answered it.
    Superseded,
    /// The window did not answer in time.
    TimedOut,
}

/// A request waiting on its window, with the way to tell its caller.
struct Parked {
    request: ProjectOpenRequest,
    outcome: oneshot::Sender<ProjectOpenOutcome>,
}

/// The requests awaiting an answer, at most one per window label.
///
/// Every outcome is sent while the lock is held, so a caller that finds its
/// request gone from the map can rely on the outcome already being in its
/// channel (see [`PendingProjectOpen::give_up`]).
#[derive(Clone, Default)]
pub struct ProjectOpenState {
    parked: Arc<Mutex<HashMap<String, Parked>>>,
}

impl ProjectOpenState {
    /// Park a request for `window_label` to show `project_id`, superseding
    /// any request that window has not answered yet.
    ///
    /// The request is pending for as long as the returned handle is held;
    /// dropping it withdraws the request.
    #[must_use = "dropping the handle withdraws the request"]
    pub fn request(&self, window_label: &str, project_id: &str) -> PendingProjectOpen {
        let request = ProjectOpenRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
        };
        let (outcome, receiver) = oneshot::channel();
        let mut parked = self.parked.lock().unwrap();
        let older = parked.insert(
            window_label.to_string(),
            Parked {
                request: request.clone(),
                outcome,
            },
        );
        if let Some(older) = older {
            let _ = older.outcome.send(ProjectOpenOutcome::Superseded);
        }
        PendingProjectOpen {
            state: self.clone(),
            window_label: window_label.to_string(),
            request,
            outcome: receiver,
        }
    }

    /// The request `window_label` has yet to answer, if any.
    pub fn pending_for(&self, window_label: &str) -> Option<ProjectOpenRequest> {
        let parked = self.parked.lock().unwrap();
        parked.get(window_label).map(|p| p.request.clone())
    }

    /// Record `window_label`'s answer to `request_id`. `false` when that is
    /// not the request the window currently holds: it was superseded, its
    /// caller stopped waiting, or it belongs to another window.
    #[must_use]
    pub fn answer(&self, window_label: &str, request_id: &str, answer: ProjectOpenAnswer) -> bool {
        let mut parked = self.parked.lock().unwrap();
        if !Self::holds(&parked, window_label, request_id) {
            return false;
        }
        parked
            .remove(window_label)
            .is_some_and(|p| p.outcome.send(ProjectOpenOutcome::Answered(answer)).is_ok())
    }

    fn holds(parked: &HashMap<String, Parked>, window_label: &str, request_id: &str) -> bool {
        parked
            .get(window_label)
            .is_some_and(|p| p.request.request_id == request_id)
    }
}

/// The caller's end of a parked request: what to tell the window, and where
/// its answer arrives.
pub struct PendingProjectOpen {
    state: ProjectOpenState,
    window_label: String,
    request: ProjectOpenRequest,
    outcome: oneshot::Receiver<ProjectOpenOutcome>,
}

impl PendingProjectOpen {
    /// The request as the window is told it.
    pub fn request(&self) -> &ProjectOpenRequest {
        &self.request
    }

    /// Wait up to `timeout` for the request to end. On timeout the request
    /// is withdrawn, so a late answer is refused rather than recorded for
    /// nobody.
    pub async fn wait(mut self, timeout: Duration) -> ProjectOpenOutcome {
        match tokio::time::timeout(timeout, &mut self.outcome).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) | Err(_) => self.give_up(),
        }
    }

    /// Stop waiting. If the request is still parked nobody ended it, and
    /// withdrawing it makes that final. If it is already gone, whoever took
    /// it sent the outcome before releasing the lock — an answer that landed
    /// as the timeout fired — so the window is not told "recorded" while the
    /// caller reads "timed out".
    fn give_up(&mut self) -> ProjectOpenOutcome {
        if self.withdraw() {
            return ProjectOpenOutcome::TimedOut;
        }
        self.outcome
            .try_recv()
            .unwrap_or(ProjectOpenOutcome::TimedOut)
    }

    /// Remove this request if it is still the one parked for its window.
    /// Returns whether it was.
    fn withdraw(&self) -> bool {
        let mut parked = self.state.parked.lock().unwrap();
        let ours = ProjectOpenState::holds(&parked, &self.window_label, &self.request.request_id);
        if ours {
            parked.remove(&self.window_label);
        }
        ours
    }
}

impl Drop for PendingProjectOpen {
    /// A caller that goes away without an outcome — the CLI was interrupted,
    /// so its handler was dropped — takes its request with it.
    fn drop(&mut self) {
        self.withdraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_answer_reaches_the_waiting_caller_and_clears_the_request() {
        let state = ProjectOpenState::default();
        let pending = state.request("main", "p1");
        let request = pending.request().clone();
        assert_eq!(request.project_id, "p1");
        assert_eq!(state.pending_for("main"), Some(request.clone()));

        let waiting = tokio::spawn(pending.wait(ANSWER_TIMEOUT));
        tokio::task::yield_now().await;
        assert!(state.answer("main", &request.request_id, ProjectOpenAnswer::Opened));

        assert_eq!(
            waiting.await.unwrap(),
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::Opened)
        );
        assert_eq!(state.pending_for("main"), None);
        assert!(
            !state.answer("main", &request.request_id, ProjectOpenAnswer::Opened),
            "a second answer has no request to land on"
        );
    }

    /// The window may answer before its caller starts waiting; the answer is
    /// held for it. A refusal is carried through as it was given.
    #[tokio::test]
    async fn an_answer_given_before_the_wait_is_kept() {
        let state = ProjectOpenState::default();
        let pending = state.request("main", "p1");
        let id = pending.request().request_id.clone();

        assert!(state.answer("main", &id, ProjectOpenAnswer::DialogOpen));

        assert_eq!(
            pending.wait(ANSWER_TIMEOUT).await,
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::DialogOpen)
        );
    }

    #[tokio::test]
    async fn a_newer_request_supersedes_the_windows_older_one() {
        let state = ProjectOpenState::default();
        let older = state.request("main", "p1");
        let older_id = older.request().request_id.clone();
        let newer = state.request("main", "p2");
        let newer_request = newer.request().clone();

        assert_eq!(
            older.wait(ANSWER_TIMEOUT).await,
            ProjectOpenOutcome::Superseded
        );
        // The older caller letting go of its handle leaves the newer request
        // parked, and the older id no longer answers anything.
        assert_eq!(state.pending_for("main"), Some(newer_request.clone()));
        assert!(!state.answer("main", &older_id, ProjectOpenAnswer::Opened));
        assert_eq!(state.pending_for("main"), Some(newer_request.clone()));

        assert!(state.answer("main", &newer_request.request_id, ProjectOpenAnswer::Opened));
        assert_eq!(
            newer.wait(ANSWER_TIMEOUT).await,
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::Opened)
        );
    }

    #[tokio::test]
    async fn windows_hold_their_requests_independently() {
        let state = ProjectOpenState::default();
        let main = state.request("main", "p1");
        let peer = state.request("win-2", "p2");
        let main_id = main.request().request_id.clone();
        let peer_request = peer.request().clone();

        assert_eq!(state.pending_for("win-2"), Some(peer_request.clone()));
        assert_eq!(state.pending_for("win-3"), None);
        assert!(
            !state.answer("win-2", &main_id, ProjectOpenAnswer::Opened),
            "a window cannot answer another window's request"
        );

        assert!(state.answer("main", &main_id, ProjectOpenAnswer::Opened));
        assert_eq!(
            main.wait(ANSWER_TIMEOUT).await,
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::Opened)
        );
        assert_eq!(
            state.pending_for("win-2"),
            Some(peer_request),
            "answering one window leaves the other's request parked"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_unanswered_request_times_out_and_is_withdrawn() {
        let state = ProjectOpenState::default();
        let pending = state.request("main", "p1");
        let id = pending.request().request_id.clone();
        let started = tokio::time::Instant::now();

        assert_eq!(
            pending.wait(ANSWER_TIMEOUT).await,
            ProjectOpenOutcome::TimedOut
        );

        assert_eq!(started.elapsed(), ANSWER_TIMEOUT);
        assert_eq!(
            state.pending_for("main"),
            None,
            "a window loading later is not offered a request nobody waits on"
        );
        assert!(
            !state.answer("main", &id, ProjectOpenAnswer::Opened),
            "a late answer is refused"
        );
    }

    /// An answer that lands as the timeout fires has already been reported
    /// to the window as recorded, so the caller must read it too.
    #[tokio::test]
    async fn giving_up_after_an_answer_landed_still_reads_the_answer() {
        let state = ProjectOpenState::default();
        let mut pending = state.request("main", "p1");
        let id = pending.request().request_id.clone();
        assert!(state.answer("main", &id, ProjectOpenAnswer::Opened));

        assert_eq!(
            pending.give_up(),
            ProjectOpenOutcome::Answered(ProjectOpenAnswer::Opened)
        );
    }

    #[test]
    fn dropping_the_handle_withdraws_the_request() {
        let state = ProjectOpenState::default();
        let pending = state.request("main", "p1");
        let id = pending.request().request_id.clone();

        drop(pending);

        assert_eq!(state.pending_for("main"), None);
        assert!(!state.answer("main", &id, ProjectOpenAnswer::Opened));
    }

    #[test]
    fn the_wire_shapes_are_camel_case() {
        let request = ProjectOpenRequest {
            request_id: "r1".to_string(),
            project_id: "p1".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({ "requestId": "r1", "projectId": "p1" })
        );

        for (wire, answer) in [
            ("opened", ProjectOpenAnswer::Opened),
            ("dialogOpen", ProjectOpenAnswer::DialogOpen),
            ("diffCommentUnsaved", ProjectOpenAnswer::DiffCommentUnsaved),
            ("projectNotFound", ProjectOpenAnswer::ProjectNotFound),
        ] {
            assert_eq!(
                serde_json::from_value::<ProjectOpenAnswer>(serde_json::json!(wire)).unwrap(),
                answer
            );
        }
        assert!(serde_json::from_value::<ProjectOpenAnswer>(serde_json::json!("ignored")).is_err());
    }
}
