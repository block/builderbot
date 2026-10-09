//! Queue gates for pipelines that may only run once another session succeeded.
//!
//! "Rebase and force push" queues its force push up front, behind the rebase,
//! with [`PipelineExecution::depends_on_session_id`] pointing at the rebase. A
//! durable queued row is what lets every client see (and cancel) the pending
//! push, and lets it survive the requesting client going away. FIFO order alone
//! is not enough, though: the drain starts the next row however the previous
//! one ended, and a force push after a failed rebase is exactly what the user
//! picked this action to avoid. So the branch drain and "Start now" both ask
//! this module before starting a row that carries a dependency.
//!
//! A `completed` status is not proof the rebase finished. The conflict handoff
//! tells the agent to leave an unresolvable rebase in progress, and that turn
//! still ends `completed`. The worktree is the authority instead: HEAD has to be
//! back on the branch, and the branch has to contain the ref it was rebased
//! onto.
//!
//! [`PipelineExecution::depends_on_session_id`]: crate::store::PipelineExecution::depends_on_session_id

use std::sync::Arc;

use crate::session_runner::SessionStatusEvent;
use crate::store::{self, PipelineKind, SessionStatus, Store};

/// What a queued row's dependency allows right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DependencyGate {
    /// No dependency, or it finished the way the row needs.
    Ready,
    /// The dependency is still queued or running. The reason is user-facing.
    Waiting(String),
    /// The dependency can no longer be satisfied, so the row should never run.
    /// The reason is user-facing and is recorded on the skipped row.
    Unmet(String),
}

fn dependency_session_id(session: &store::Session) -> Option<&str> {
    session.pipeline.as_ref()?.depends_on_session_id.as_deref()
}

fn is_rebase(session: &store::Session) -> bool {
    session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.kind.as_ref())
        == Some(&PipelineKind::Rebase)
}

/// How the skip reason names the dependency.
fn dependency_noun(dependency: &store::Session) -> &'static str {
    if is_rebase(dependency) {
        "rebase"
    } else {
        "session it was waiting on"
    }
}

/// The gate as far as the dependency's status alone can answer it.
///
/// `Ok(None)` means the dependency completed and its outcome still has to be
/// checked against the worktree (see [`evaluate_dependency_gate`]).
fn status_gate(dependency: Option<&store::Session>) -> Option<DependencyGate> {
    let Some(dependency) = dependency else {
        return Some(DependencyGate::Unmet(
            "Skipped because the session it was waiting on no longer exists.".to_string(),
        ));
    };
    let noun = dependency_noun(dependency);
    match dependency.status {
        SessionStatus::Queued | SessionStatus::Running => Some(DependencyGate::Waiting(format!(
            "This waits for the {noun} to finish first."
        ))),
        SessionStatus::Error => Some(DependencyGate::Unmet(format!(
            "Skipped because the {noun} failed."
        ))),
        SessionStatus::Cancelled => Some(DependencyGate::Unmet(format!(
            "Skipped because the {noun} was cancelled."
        ))),
        SessionStatus::Completed => None,
    }
}

/// Decide whether a queued row's dependency lets it start.
///
/// Rows without a dependency are always `Ready`. A completed rebase dependency
/// is checked against the branch's checkout, which on a remote branch is a
/// workspace round trip, so this runs the git reads on a blocking thread.
pub(crate) async fn evaluate_dependency_gate(
    store: &Arc<Store>,
    branch_id: &str,
    session: &store::Session,
) -> Result<DependencyGate, String> {
    let Some(dependency_id) = dependency_session_id(session) else {
        return Ok(DependencyGate::Ready);
    };
    let dependency = store
        .get_session(dependency_id)
        .map_err(|e| e.to_string())?;
    if let Some(gate) = status_gate(dependency.as_ref()) {
        return Ok(gate);
    }
    let dependency = dependency.expect("status_gate answers for a missing dependency");
    if !is_rebase(&dependency) {
        return Ok(DependencyGate::Ready);
    }

    let rebase_target = dependency
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.rebase_target.clone());
    let store = Arc::clone(store);
    let branch_id = branch_id.to_string();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        check_rebase_landed(&store, &branch_id, rebase_target.as_deref())
    })
    .await
    .map_err(|e| format!("Rebase outcome check failed: {e}"))?;

    Ok(match outcome {
        Ok(()) => DependencyGate::Ready,
        Err(reason) => DependencyGate::Unmet(reason),
    })
}

/// Check the branch's own checkout for a finished rebase onto
/// `origin/<rebase_target>` (or the branch's base when `None`).
///
/// `Err` carries the user-facing skip reason. A check that cannot run reads as
/// unfinished: pushing on a guess is the direction this gate exists to avoid.
fn check_rebase_landed(
    store: &Arc<Store>,
    branch_id: &str,
    rebase_target: Option<&str>,
) -> Result<(), String> {
    let unconfirmed = |e: String| format!("Skipped because the rebase could not be confirmed: {e}");
    let ctx = crate::prs::resolve_branch_pipeline_context(store, branch_id).map_err(unconfirmed)?;
    let target_ref = crate::git::origin_ref_for_branch(
        rebase_target
            .unwrap_or_else(|| crate::git::branch_name_without_origin(&ctx.branch.base_branch)),
    );
    let branch_name = crate::git::branch_name_without_origin(&ctx.branch.branch_name);
    let git = crate::branches::git_runner(
        &ctx.working_dir,
        ctx.workspace_name.as_deref(),
        ctx.remote_working_dir
            .as_deref()
            .and_then(|dir| dir.to_str())
            .map(str::to_string),
    );
    rebase_outcome(&git, branch_name, &target_ref)
}

/// The worktree half of [`check_rebase_landed`], over any git runner.
///
/// Every rebase backend detaches HEAD while it works, so HEAD back on the
/// branch means no rebase is stopped mid-way. Containing the target is what
/// tells a finished rebase from one the agent aborted (HEAD is back on the
/// branch then too, unmoved) or one whose fetch never recovered.
fn rebase_outcome<F>(git: &F, branch_name: &str, target_ref: &str) -> Result<(), String>
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    let on_branch = git(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .is_ok_and(|head| head.trim() == branch_name);
    if !on_branch {
        return Err(format!(
            "Skipped because the rebase did not finish: HEAD is not on {branch_name}, so the rebase is likely still in progress."
        ));
    }

    let range = format!("HEAD..{target_ref}");
    let missing = git(&["rev-list", "--count", &range])
        .map_err(|e| format!("Skipped because the rebase could not be confirmed: {e}"))?;
    if missing.trim() != "0" {
        return Err(format!(
            "Skipped because the rebase did not finish: {branch_name} is not on top of {target_ref}."
        ));
    }
    Ok(())
}

/// The `sessionType` a skipped row's status event carries, matching what its
/// running event would have said.
fn session_type_for(session: &store::Session) -> &'static str {
    match session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.kind.as_ref())
    {
        Some(PipelineKind::Push) => "push",
        Some(PipelineKind::Pull) => "pull",
        Some(PipelineKind::Rebase | PipelineKind::Squash) | None => "commit",
    }
}

/// Cancel a queued row whose dependency is unmet, recording `reason`, and tell
/// every client.
///
/// Returns whether this call cancelled it. `false` means the row already left
/// the queue (a concurrent start or cancel won), which the caller treats like
/// any other row that moved under it.
pub(crate) fn skip_dependent_session<R: tauri::Runtime>(
    store: &Store,
    app_handle: &tauri::AppHandle<R>,
    session: &store::Session,
    branch_id: &str,
    reason: &str,
) -> Result<bool, String> {
    let skipped = store
        .transition_from_queued(&session.id, SessionStatus::Cancelled, Some(reason), None)
        .map_err(|e| e.to_string())?;
    if !skipped {
        return Ok(false);
    }

    log::info!(
        "Skipped queued session {} on branch {branch_id}: {reason}",
        session.id
    );
    let project_id = store
        .get_branch(branch_id)
        .ok()
        .flatten()
        .map(|branch| branch.project_id);
    crate::web_server::emit_to_all(
        app_handle,
        "session-status-changed",
        SessionStatusEvent {
            session_id: session.id.clone(),
            status: "cancelled".to_string(),
            error_message: Some(reason.to_string()),
            completion_reason: None,
            branch_id: Some(branch_id.to_string()),
            project_id,
            session_type: Some(session_type_for(session).to_string()),
        },
    );
    Ok(true)
}

/// Skip the queued rows waiting on a dependency that was just cancelled while
/// still queued.
///
/// Cancelling a queued row writes its status without draining the branch, so
/// without this the dependent row would sit until some unrelated session on
/// the branch finished — and, being queued, keep every new request on the
/// branch queued behind it in the meantime.
pub(crate) fn skip_dependents_of_cancelled<R: tauri::Runtime>(
    store: &Store,
    app_handle: &tauri::AppHandle<R>,
    dependency_id: &str,
) -> Result<(), String> {
    let Some(branch_id) = store
        .get_branch_id_for_session(dependency_id)
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let dependency = store
        .get_session(dependency_id)
        .map_err(|e| e.to_string())?;
    let Some(DependencyGate::Unmet(reason)) = status_gate(dependency.as_ref()) else {
        return Ok(());
    };

    let queued = store
        .get_queued_sessions_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;
    for dependent in queued
        .iter()
        .filter(|session| dependency_session_id(session) == Some(dependency_id))
    {
        skip_dependent_session(store, app_handle, dependent, &branch_id, &reason)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn rebase_session(status: SessionStatus) -> store::Session {
        let mut session = store::Session::new_running("Rebase branch", Path::new("/tmp"));
        session.status = status;
        session.pipeline =
            Some(store::PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
        session
    }

    #[test]
    fn status_gate_waits_on_active_and_refuses_failed_dependencies() {
        assert!(matches!(
            status_gate(Some(&rebase_session(SessionStatus::Queued))),
            Some(DependencyGate::Waiting(_))
        ));
        assert!(matches!(
            status_gate(Some(&rebase_session(SessionStatus::Running))),
            Some(DependencyGate::Waiting(_))
        ));
        assert_eq!(
            status_gate(Some(&rebase_session(SessionStatus::Error))),
            Some(DependencyGate::Unmet(
                "Skipped because the rebase failed.".to_string()
            ))
        );
        assert_eq!(
            status_gate(Some(&rebase_session(SessionStatus::Cancelled))),
            Some(DependencyGate::Unmet(
                "Skipped because the rebase was cancelled.".to_string()
            ))
        );
        assert!(matches!(status_gate(None), Some(DependencyGate::Unmet(_))));
        // Completed still needs the worktree check.
        assert_eq!(
            status_gate(Some(&rebase_session(SessionStatus::Completed))),
            None
        );
    }

    /// A scripted git runner: `symbolic-ref` answers `head` (or fails when
    /// `None`, as it does on a detached HEAD), and `rev-list --count` answers
    /// `missing`.
    fn git(
        head: Option<&'static str>,
        missing: &'static str,
    ) -> impl Fn(&[&str]) -> Result<String, String> {
        move |args: &[&str]| match args.first().copied() {
            Some("symbolic-ref") => head
                .map(|h| format!("{h}\n"))
                .ok_or_else(|| "fatal: ref HEAD is not a symbolic ref".to_string()),
            Some("rev-list") => {
                assert_eq!(args, ["rev-list", "--count", "HEAD..origin/main"]);
                Ok(format!("{missing}\n"))
            }
            other => panic!("unexpected git call {other:?}"),
        }
    }

    #[test]
    fn finished_rebase_is_on_the_branch_and_contains_the_target() {
        assert_eq!(
            rebase_outcome(&git(Some("feature"), "0"), "feature", "origin/main"),
            Ok(())
        );
    }

    #[test]
    fn rebase_left_in_progress_is_refused() {
        let reason = rebase_outcome(&git(None, "0"), "feature", "origin/main").unwrap_err();
        assert!(reason.contains("still in progress"), "{reason}");
        // HEAD attached to some other branch is just as unsafe to push from.
        assert!(rebase_outcome(&git(Some("main"), "0"), "feature", "origin/main").is_err());
    }

    #[test]
    fn aborted_or_unfetched_rebase_is_refused() {
        // `git rebase --abort` puts HEAD back on the branch, unmoved, so only
        // the missing target commits give it away.
        let reason =
            rebase_outcome(&git(Some("feature"), "3"), "feature", "origin/main").unwrap_err();
        assert!(reason.contains("not on top of origin/main"), "{reason}");
    }

    #[test]
    fn unreadable_rebase_outcome_is_refused() {
        let failing = |args: &[&str]| match args.first().copied() {
            Some("symbolic-ref") => Ok("feature\n".to_string()),
            _ => Err("fatal: bad revision".to_string()),
        };
        assert!(rebase_outcome(&failing, "feature", "origin/main").is_err());
    }

    struct GatedPush {
        store: Arc<Store>,
        branch: store::Branch,
        rebase: store::Session,
        push: store::Session,
        repo: crate::test_utils::TempGitRepo,
    }

    /// A local branch `feature` on a real repo, a rebase session linked to it
    /// through its pending commit, and a queued force push gated on that rebase.
    fn gated_push(rebase_status: SessionStatus) -> GatedPush {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("base.txt", "base\n");
        let base = repo.commit("chore: base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("feature.txt", "feature\n");
        repo.commit("feat: feature");

        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let workdir = store::Workdir::new(&project.id, &repo.path().to_string_lossy())
            .with_branch(&branch.id);
        store.create_workdir(&workdir).unwrap();

        let rebase = rebase_session(rebase_status);
        store.create_session(&rebase).unwrap();
        store
            .create_commit(&store::Commit::new_pending(&branch.id).with_session(&rebase.id))
            .unwrap();

        let mut push = store::Session::new_queued("Force push the current branch to the remote")
            .with_branch(&branch.id);
        push.pipeline = Some(
            store::PipelineExecution::from_steps(&[])
                .with_kind(PipelineKind::Push)
                .with_push_force(true)
                .with_depends_on_session_id(Some(rebase.id.clone())),
        );
        store.create_session(&push).unwrap();

        GatedPush {
            store,
            branch,
            rebase,
            push,
            repo,
        }
    }

    fn gate(fixture: &GatedPush) -> DependencyGate {
        tauri::async_runtime::block_on(evaluate_dependency_gate(
            &fixture.store,
            &fixture.branch.id,
            &fixture.push,
        ))
        .unwrap()
    }

    #[test]
    fn rows_without_a_dependency_are_ready() {
        let fixture = gated_push(SessionStatus::Running);
        let plain = store::Session::new_queued("Push").with_branch(&fixture.branch.id);
        assert_eq!(
            tauri::async_runtime::block_on(evaluate_dependency_gate(
                &fixture.store,
                &fixture.branch.id,
                &plain,
            ))
            .unwrap(),
            DependencyGate::Ready
        );
    }

    #[test]
    fn push_waits_while_its_rebase_is_running() {
        assert!(matches!(
            gate(&gated_push(SessionStatus::Running)),
            DependencyGate::Waiting(_)
        ));
    }

    #[test]
    fn completed_rebase_that_landed_releases_the_push() {
        assert_eq!(
            gate(&gated_push(SessionStatus::Completed)),
            DependencyGate::Ready
        );
    }

    /// The case a status-only gate gets wrong: the rebase session ended
    /// `completed`, but the branch never made it onto the new base.
    #[test]
    fn completed_rebase_that_did_not_land_skips_the_push() {
        let fixture = gated_push(SessionStatus::Completed);
        let repo = &fixture.repo;
        repo.run_git(&["checkout", "main"]);
        repo.write_file("upstream.txt", "upstream\n");
        let upstream = repo.commit("feat: upstream");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &upstream]);
        repo.run_git(&["checkout", "feature"]);

        let DependencyGate::Unmet(reason) = gate(&fixture) else {
            panic!("a branch behind its rebase target must not be force pushed");
        };
        assert!(reason.contains("not on top of origin/main"), "{reason}");

        // Detached mid-rebase is refused before the target is even consulted.
        repo.run_git(&["checkout", "--detach", "HEAD"]);
        let DependencyGate::Unmet(reason) = gate(&fixture) else {
            panic!("a detached HEAD must not be force pushed");
        };
        assert!(reason.contains("still in progress"), "{reason}");
    }

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app")
    }

    #[test]
    fn cancelling_a_queued_rebase_skips_the_push_waiting_on_it() {
        let fixture = gated_push(SessionStatus::Cancelled);
        let app = mock_app();

        skip_dependents_of_cancelled(&fixture.store, app.handle(), &fixture.rebase.id).unwrap();

        let push = fixture
            .store
            .get_session(&fixture.push.id)
            .unwrap()
            .unwrap();
        assert_eq!(push.status, SessionStatus::Cancelled);
        assert_eq!(
            push.error_message.as_deref(),
            Some("Skipped because the rebase was cancelled.")
        );
        assert!(fixture
            .store
            .get_queued_sessions_for_branch(&fixture.branch.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn skipping_a_row_that_already_left_the_queue_is_a_no_op() {
        let fixture = gated_push(SessionStatus::Error);
        let app = mock_app();
        assert!(fixture
            .store
            .transition_queued_to_running(&fixture.push.id)
            .unwrap());

        assert!(!skip_dependent_session(
            &fixture.store,
            app.handle(),
            &fixture.push,
            &fixture.branch.id,
            "Skipped because the rebase failed.",
        )
        .unwrap());
        assert_eq!(
            fixture
                .store
                .get_session(&fixture.push.id)
                .unwrap()
                .unwrap()
                .status,
            SessionStatus::Running
        );
    }
}
