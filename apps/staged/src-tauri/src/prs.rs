use serde::Serialize;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::git;
use crate::session_commands::{
    fail_claimed_queued_start, BranchSessionLaunchStatus, QueuedSessionClaim, QueuedSessionStart,
};
use crate::session_runner;
use crate::store::{
    self, FailureStrategy, PipelineExecution, PipelineKind, PipelineStep, StepStatus, Store,
};

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

pub(crate) fn resolve_branch_repo_and_subpath(
    store: &Arc<Store>,
    project: &store::Project,
    branch: &store::Branch,
) -> Result<(String, Option<String>), String> {
    if let Some(repo_id) = &branch.project_repo_id {
        if let Some(repo) = store.get_project_repo(repo_id).map_err(|e| e.to_string())? {
            return Ok((repo.github_repo, repo.subpath));
        }
    }

    let repo_slug = project
        .primary_repo()
        .ok_or_else(|| format!("Project '{}' has no repository attached", project.name))?;
    Ok((repo_slug.to_string(), project.subpath.clone()))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrStatusEvent {
    pub(crate) branch_id: String,
    pub(crate) pr_state: String,
    pub(crate) pr_checks_status: String,
    pub(crate) pr_review_decision: Option<String>,
    pub(crate) pr_mergeable: bool,
    pub(crate) pr_draft: bool,
    pub(crate) pr_head_sha: Option<String>,
    pub(crate) pr_fetched_at: i64,
    pub(crate) failed_checks: Vec<git::FailedCheck>,
}

// =============================================================================
// Pipeline session helper
// =============================================================================

/// Result of a start-or-queue branch pipeline command.
///
/// Rebase and squash can be requested while the branch already has work in
/// flight, so the caller needs to know whether the returned session started
/// running or is waiting its turn on the branch queue.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchPipelineResponse {
    pub session_id: String,
    pub session_status: BranchSessionLaunchStatus,
    /// The queued session that runs after this one, and only if it succeeds
    /// (the force push of "Rebase and force push"). Always queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_up_session_id: Option<String>,
}

impl BranchPipelineResponse {
    fn running(session_id: String) -> Self {
        Self {
            session_id,
            session_status: BranchSessionLaunchStatus::Running,
            follow_up_session_id: None,
        }
    }

    fn queued(session_id: String) -> Self {
        Self {
            session_id,
            session_status: BranchSessionLaunchStatus::Queued,
            follow_up_session_id: None,
        }
    }
}

/// Resolved context for a branch, ready to start a pipeline session.
pub(crate) struct BranchPipelineContext {
    pub(crate) branch: store::Branch,
    pub(crate) working_dir: PathBuf,
    pub(crate) workspace_name: Option<String>,
    pub(crate) remote_working_dir: Option<PathBuf>,
}

/// Resolve branch, project, and working directory for a pipeline command.
///
/// All pipeline commands (create_pr, push_branch, rebase_branch, squash_commits)
/// share the same setup: look up the branch and project, resolve the working
/// directory (local worktree vs remote clone), and compute the remote working
/// directory for workspace-based branches.
pub(crate) fn resolve_branch_pipeline_context(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<BranchPipelineContext, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let (repo_slug, repo_subpath) = resolve_branch_repo_and_subpath(store, &project, &branch)?;

    let is_remote = branch.branch_type == store::BranchType::Remote;

    let (working_dir, workspace_name) = if is_remote {
        let clone_path = crate::paths::repos_dir()
            .map(|d| d.join(&repo_slug))
            .ok_or_else(|| "Cannot determine clone path for remote branch".to_string())?;
        (clone_path, branch.workspace_name.clone())
    } else {
        let workdir = store
            .get_workdir_for_branch(branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut working_dir = PathBuf::from(&workdir.path);
        if let Some(subpath) = repo_subpath {
            working_dir = working_dir.join(subpath);
        }
        (working_dir, None)
    };

    // Resolve the actual workspace path for remote branches so the remote
    // agent starts in the correct repo directory.
    let remote_working_dir = if is_remote {
        branch
            .workspace_name
            .as_deref()
            .and_then(|ws| {
                crate::branches::resolve_branch_workspace_subpath(store, &branch)
                    .ok()
                    .flatten()
                    .and_then(|subpath| {
                        crate::branches::resolve_workspace_repo_path(ws, &subpath).ok()
                    })
            })
            .map(PathBuf::from)
    } else {
        None
    };

    Ok(BranchPipelineContext {
        branch,
        working_dir,
        workspace_name,
        remote_working_dir,
    })
}

/// Create a session with a pipeline and start it in the background.
///
/// Handles session creation, pipeline persistence, the "running" event emission,
/// and spawning the pipeline runner. Returns the session ID.
#[allow(clippy::too_many_arguments)]
fn start_pipeline_for_branch(
    ctx: BranchPipelineContext,
    steps: Vec<PipelineStep>,
    prompt: &str,
    session_type: &str,
    provider: Option<String>,
    store: Arc<Store>,
    app_handle: &tauri::AppHandle,
    registry: &Arc<session_runner::SessionRegistry>,
) -> Result<String, String> {
    crate::project_transfer::ensure_branch_not_transferring(&ctx.branch.id)?;
    let pipeline = PipelineExecution::from_steps(&steps);

    // pr/push pipeline sessions link no artifact row, so the session row
    // itself carries the branch linkage — busy-state snapshots and terminal
    // completion side-effects resolve the branch from it.
    let mut session =
        store::Session::new_running(prompt, &ctx.working_dir).with_branch(&ctx.branch.id);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    // Live from before the `running` write until the runner has registered
    // (see `SessionRegistry::hold`); released when this function returns.
    let _liveness = registry.hold(&session.id);
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Emit "running" event *before* returning so the global session listener
    // registers this session atomically — avoiding the race where the session
    // completes before the frontend `.then()` callback fires.
    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

    session_runner::emit_session_running(
        app_handle,
        &session.id,
        &branch_id,
        &project_id,
        session_type,
    );

    session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: session.id.clone(),
            prompt: prompt.to_string(),
            steps,
            pipeline,
            working_dir: ctx.working_dir,
            pre_head_sha: None,
            provider,
            workspace_name: ctx.workspace_name,
            remote_working_dir: ctx.remote_working_dir,
            branch_id: Some(branch_id),
            project_id: Some(project_id),
        },
        store,
        app_handle.clone(),
        Arc::clone(registry),
    )?;

    Ok(session.id)
}

/// Derive the base branch name, stripping the `origin/` prefix if present.
///
/// GitHub's PR API expects the bare branch name, while local comparison
/// commands should use `git::origin_ref_for_branch` so the stale local base
/// branch is never consulted.
fn base_branch_name(branch: &store::Branch) -> &str {
    git::branch_name_without_origin(&branch.base_branch)
}

/// Determine the remote ref to rebase onto based on the `target` parameter.
///
/// - `None` or `Some("base")` → base branch name (e.g. `main`)
/// - `Some("origin")` → `remote_branch`, the branch on origin the checkout
///   pushes to (the PR head rather than the local name when the two differ)
fn rebase_ref_for_target(
    branch: &store::Branch,
    remote_branch: &str,
    target: Option<&str>,
) -> String {
    match target {
        Some("origin") => remote_branch.to_string(),
        _ => base_branch_name(branch).to_string(),
    }
}

// Every push prompt must contain "Push the current branch to the remote" or
// "Force push the current branch to the remote":
// `infer_branch_resume_session_type` matches on those to recognise a push
// session, and without a match no `push-completed` event fires.
const PUSH_PROMPT: &str = "Push the current branch to the remote with a normal push. If the push fails for a recoverable reason, diagnose and fix it, then retry with a normal push. Do not force push.";
const FORCE_PUSH_PROMPT: &str = "Force push the current branch to the remote";
const NO_VERIFY_PUSH_PROMPT: &str = "Push the current branch to the remote with a normal push, skipping the pre-push hook (`--no-verify`). If the push fails for a recoverable reason, diagnose and fix it, then retry with a normal push that keeps `--no-verify`. Do not force push.";
const NO_VERIFY_FORCE_PUSH_PROMPT: &str =
    "Force push the current branch to the remote, skipping the pre-push hook (`--no-verify`)";
/// A pull never hands off to an agent (every step aborts), so this is purely the
/// session's label.
const PULL_PROMPT: &str = "Pull from origin";

/// What a push pipeline does: whether it force-pushes (`--force-with-lease`),
/// and whether it skips git's `pre-push` hook (`--no-verify`).
///
/// Persisted on the pipeline as `push_force` / `push_no_verify`, so a queued
/// push re-derives the same command on dequeue. Every combination is its own
/// operation: the queue never folds one variant into another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PushVariant {
    pub force: bool,
    pub no_verify: bool,
}

impl PushVariant {
    /// The variant a persisted pipeline records. Non-push pipelines record
    /// neither flag, so they read as the default.
    fn of(pipeline: &PipelineExecution) -> Self {
        Self {
            force: pipeline.push_force,
            no_verify: pipeline.push_no_verify,
        }
    }

    /// The variant a `push_branch` request asks for; an omitted flag is off.
    pub(crate) fn from_request(force: Option<bool>, no_verify: Option<bool>) -> Self {
        Self {
            force: force.unwrap_or(false),
            no_verify: no_verify.unwrap_or(false),
        }
    }

    /// Flags shared by both halves of [`git_push_with_fallback`].
    fn git_flags(self) -> &'static str {
        if self.no_verify {
            "--no-verify "
        } else {
            ""
        }
    }
}

/// Prompt for a pipeline session, which doubles as its timeline label.
///
/// `push` only matters for [`PipelineKind::Push`]; the other kinds ignore it.
/// Queued and running paths share this so a session's label doesn't change
/// when it is drained.
fn pipeline_prompt(kind: &PipelineKind, push: PushVariant) -> &'static str {
    match kind {
        PipelineKind::Rebase => "Rebase branch",
        PipelineKind::Squash => "Squash commits",
        PipelineKind::Push => match (push.force, push.no_verify) {
            (false, false) => PUSH_PROMPT,
            (true, false) => FORCE_PUSH_PROMPT,
            (false, true) => NO_VERIFY_PUSH_PROMPT,
            (true, true) => NO_VERIFY_FORCE_PUSH_PROMPT,
        },
        PipelineKind::Pull => PULL_PROMPT,
    }
}

/// The rebase target to persist on the pipeline, or `None` when the pipeline
/// targets the branch's configured base.
///
/// Only "rebase onto origin" needs a persisted target; a base rebase re-derives
/// it from the branch on dequeue. Keeping this in one place means the queued and
/// running paths agree, which the same-kind dedupe check relies on.
fn persisted_rebase_target(target: Option<&str>, rebase_ref: &str) -> Option<String> {
    matches!(target, Some("origin")).then(|| rebase_ref.to_string())
}

/// Find a queued pipeline session on this branch that already performs exactly
/// the work being requested, so a second click doesn't stack a duplicate.
///
/// `matches` has to compare every persisted field that changes what the pipeline
/// does, not just the kind: "rebase onto base" vs "rebase onto origin" and
/// "push" vs "force push" are different operations that share a [`PipelineKind`].
fn find_queued_pipeline(
    store: &Arc<Store>,
    branch_id: &str,
    matches: impl Fn(&PipelineExecution) -> bool,
) -> Result<Option<String>, String> {
    let queued = store
        .get_queued_sessions_for_branch(branch_id)
        .map_err(|e| e.to_string())?;

    Ok(queued
        .into_iter()
        .find(|session| session.pipeline.as_ref().is_some_and(&matches))
        .map(|session| session.id))
}

/// Whether the branch already has work in flight, and a new request therefore
/// has to join the queue instead of starting now.
///
/// Callers must hold the branch launch lock: the answer is only meaningful for
/// as long as no session can start or be enqueued underneath them.
fn branch_has_work_in_flight(store: &Arc<Store>, branch_id: &str) -> Result<bool, String> {
    Ok(store
        .has_running_session_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        || !store
            .get_queued_sessions_for_branch(branch_id)
            .map_err(|e| e.to_string())?
            .is_empty())
}

const HTTPS_FALLBACK_CONFIG: &str = "url.https://github.com/.insteadOf=git@github.com:";

fn git_fetch_with_fallback(refspec: &str) -> String {
    format!(
        "if ! git fetch origin {refspec}; then git -c '{HTTPS_FALLBACK_CONFIG}' fetch origin {refspec}; fi"
    )
}

/// `local:remote` refspec for `git push`. Spelled out even when the names
/// match so every push site reads the same way.
fn push_refspec(local_branch: &str, remote_branch: &str) -> String {
    format!("{local_branch}:{remote_branch}")
}

fn git_push_with_fallback(args: &str) -> String {
    format!("if ! git push {args}; then git -c '{HTTPS_FALLBACK_CONFIG}' push {args}; fi")
}

/// Build the steps for a rebase or squash pipeline.
///
/// `base_branch` is the branch's configured upstream base (e.g. `main`). For
/// squash this is also the bound the reset must not cross. For rebase it is
/// purely informational in the AI handoff prompts so the agent can sanity-check
/// the requested target.
///
/// `rebase_target` is the remote ref the rebase will actually run against. It
/// equals `base_branch` for the default "rebase onto base" action and equals
/// the branch's own remote branch for "rebase onto origin" (used when local
/// has diverged from `origin/{branch}`). Only the rebase variant consults this
/// value; squash always operates against the base branch.
///
/// Every rebase variant runs [`git::rebase_preserving_merge_edits_command`]
/// rather than a bare `git rebase`, so merge commits that carry hand edits are
/// linearised into ordinary commits instead of being silently dropped.
///
/// Errors for [`PipelineKind::Push`] and [`PipelineKind::Pull`], which produce no
/// commit and belong to the git pipeline path — the only caller that reads the
/// kind from the database checks it first, so this guards against a misrouted
/// queued session rather than a reachable input.
fn build_commit_pipeline_steps(
    kind: &PipelineKind,
    base_branch: &str,
    rebase_target: &str,
) -> Result<Vec<PipelineStep>, String> {
    let steps = match kind {
        PipelineKind::Rebase => {
            let target_note = if base_branch == rebase_target {
                String::new()
            } else {
                format!(
                    " The branch's configured base is `origin/{base_branch}`; the requested target `origin/{rebase_target}` is different. If `origin/{rebase_target}` is itself behind `origin/{base_branch}` by a non-trivial amount, surface that to the user before continuing — the rebase target may be wrong."
                )
            };
            let (fetch_label, rebase_label) = if base_branch == rebase_target {
                ("Fetch latest base".to_string(), "Rebase onto base".to_string())
            } else {
                (
                    format!("Fetch origin/{rebase_target}"),
                    format!("Rebase onto origin/{rebase_target}"),
                )
            };
            vec![
                PipelineStep::Command {
                    label: fetch_label,
                    command: git_fetch_with_fallback(rebase_target),
                    on_failure: FailureStrategy::HandoffToAi {
                        prompt_template: format!(
                            "The fetch failed. Diagnose and fix the issue, then rebase this branch onto `origin/{rebase_target}` with DCO signoffs. Resolve conflicts if present and continue the rebase. Do not push the branch.{target_note}\n\n{{step_outputs}}"
                        ),
                    },
                },
                PipelineStep::Command {
                    label: rebase_label,
                    command: git::rebase_preserving_merge_edits_command(&format!(
                        "origin/{rebase_target}"
                    )),
                    on_failure: FailureStrategy::HandoffToAi {
                        prompt_template: format!(
                            "The rebase onto `origin/{rebase_target}` failed. Inspect the output and recover from the actual failure. If the rebase is stopped on a conflict, resolve it, stage the resolution, and run `git rebase --continue` (the rebase already applies DCO signoffs) until it finishes. Never restart the rebase and never run a plain `git rebase`: commits titled \"{hand_edit_prefix}<sha>\" carry content that existed only inside that merge commit and is in no other commit, so a fresh rebase would lose it. Resolve conflicts in those commits so the content is kept, and never drop or skip them. `git rebase --abort` is the user's decision, not yours; if you cannot resolve a conflict, leave the rebase in progress and report why. If the output says the rebase was refused before it started (for example because git is too old), do not attempt it another way; report the reason to the user. Do not push the branch.{target_note}\n\n{{step_outputs}}",
                            hand_edit_prefix = git::HAND_EDIT_COMMIT_SUBJECT_PREFIX,
                        ),
                    },
                },
            ]
        }
        PipelineKind::Squash => vec![
            // Fetch first so origin/{base} is up-to-date before computing the
            // merge-base for the destructive soft reset. Without this, a stale
            // remote-tracking ref could cause the reset to target the wrong commit.
            PipelineStep::Command {
                label: "Fetch latest base".to_string(),
                command: git_fetch_with_fallback(base_branch),
                on_failure: FailureStrategy::HandoffToAi {
                    prompt_template: format!(
                        "The fetch failed. Diagnose and fix the issue, then squash only this branch's commits manually. Do not squash beyond the merge-base with `origin/{base_branch}`. Create one signed-off conventional commit from the staged changes and do not push the branch.\n\n{{step_outputs}}"
                    ),
                },
            },
            PipelineStep::Command {
                label: "View commit history".to_string(),
                command: format!("git log --oneline origin/{base_branch}..HEAD"),
                on_failure: FailureStrategy::Continue,
            },
            PipelineStep::Command {
                label: "Soft reset to merge-base".to_string(),
                command: format!(
                    r#"merge_base=$(git merge-base origin/{base_branch} HEAD) && git reset --soft "$merge_base""#
                ),
                on_failure: FailureStrategy::HandoffToAi {
                    prompt_template: format!(
                        "The soft reset failed. Diagnose and fix the issue, then squash only this branch's commits manually. Do not squash beyond the merge-base with `origin/{base_branch}`. Create one signed-off conventional commit from the staged changes and do not push the branch.\n\n{{step_outputs}}"
                    ),
                },
            },
            PipelineStep::AiHandoff {
                label: "Write squashed commit message".to_string(),
                prompt_template: r#"<action>
Create a single conventional-commit from the staged changes.

The branch's commits have been soft-reset to the merge-base and are staged. Now create one commit:
- The commit message MUST use conventional commit style (e.g., "feat: add user authentication", "fix: resolve null pointer in parser")
- Choose the most appropriate conventional commit type (feat, fix, refactor, docs, style, test, chore, perf, ci, build) based on the actual changes
- Use the user's global git identity for author and committer
- Create the commit with DCO signoff (`git commit --signoff`)
- Use the original commit history (from step 2) as context to write a meaningful message
- Do NOT push the branch

Here is the context from the prior steps:

{step_outputs}
</action>"#
                    .to_string(),
            },
        ],
        PipelineKind::Push | PipelineKind::Pull => {
            return Err(format!("{kind:?} is not a commit pipeline"));
        }
    };

    Ok(steps)
}

/// Build the single step that pushes the branch to its remote.
///
/// Rebuilt from the persisted `push_force` / `push_no_verify` flags on dequeue
/// rather than replayed from the queued pipeline, so a queued push always
/// pushes the branch's current name with the command the user asked for.
///
/// The refspec is always explicit (`local:remote`) so a de-duplicated local
/// name (`feature-x-2`) still lands on the PR's branch (`feature-x`) instead of
/// creating a new remote branch; `-u` then tracks `origin/{remote}`.
///
/// `--no-verify` skips only git's `pre-push` hook; server-side checks still
/// apply. It goes into both halves of the HTTPS fallback, so the retry doesn't
/// run the hook either.
pub(crate) fn build_push_pipeline_steps(
    local_branch: &str,
    remote_branch: &str,
    variant: PushVariant,
) -> Vec<PipelineStep> {
    let refspec = push_refspec(local_branch, remote_branch);
    let flags = variant.git_flags();
    let push_command = if variant.force {
        git_push_with_fallback(&format!("{flags}-u origin {refspec} --force-with-lease"))
    } else {
        git_push_with_fallback(&format!("{flags}-u origin {refspec}"))
    };

    let on_failure = if variant.force {
        // Spell the refspec out: with the upstream on `origin/{remote}` a
        // bare `git push` is refused under `push.default=simple` when the
        // names differ, and the obvious fallback `git push origin {local}`
        // would recreate the stray remote branch the refspec avoids. The
        // flags are spelled out too, or the agent's retry runs the hook the
        // user asked to skip.
        FailureStrategy::HandoffToAi {
            prompt_template: format!(
                "The force push failed. Diagnose and fix the issue, then retry the force push with exactly `git push {flags}-u origin {refspec} --force-with-lease`. Do not push `{local_branch}` under any other remote name.\n\n{{step_outputs}}"
            ),
        }
    } else {
        // For normal push, abort on non-fast-forward so the frontend can show
        // the force-push dialog. The marker matches git's actual stderr output
        // (e.g. "! [rejected] main -> main (non-fast-forward)").
        //
        // If the push fails for a *different* reason (e.g. auth error, network
        // timeout), the marker won't match and the pipeline falls through to an
        // AI handoff for generic diagnosis — this is intentional.
        FailureStrategy::Abort {
            marker: Some("non-fast-forward".to_string()),
        }
    };

    vec![PipelineStep::Command {
        label: "Push to remote".to_string(),
        command: push_command,
        on_failure,
    }]
}

/// Build the steps for a queued fast-forward pull.
///
/// Both steps abort rather than handing off to an agent: a failed `--ff-only`
/// merge means the branch diverged while the pull waited, and the fix is a user
/// decision (rebase onto origin, or reset to origin) rather than something an
/// agent should pick. `session_runner` turns that abort into a session error the
/// frontend toasts, since a drained pull has no one watching it.
///
/// Rebuilt from the branch's current remote branch on dequeue, like the push
/// steps.
fn build_pull_pipeline_steps(remote_branch: &str) -> Vec<PipelineStep> {
    vec![
        PipelineStep::Command {
            label: format!("Fetch origin/{remote_branch}"),
            command: git_fetch_with_fallback(remote_branch),
            on_failure: FailureStrategy::Abort { marker: None },
        },
        PipelineStep::Command {
            label: format!("Fast-forward to origin/{remote_branch}"),
            command: format!("git merge --ff-only origin/{remote_branch}"),
            on_failure: FailureStrategy::Abort { marker: None },
        },
    ]
}

/// The rows a run-now pipeline needs before the session runner can pick it up.
///
/// Inserted under the branch launch lock and consumed by
/// [`launch_running_pipeline_session`] once the lock is released.
///
/// Carries the row's liveness hold (see `SessionRegistry::hold`), taken
/// before the insert and released when this is dropped — after the runner
/// has registered the session, for a launch; after the terminal write, for a
/// rebase that failed before launching.
struct RunningPipelineSession {
    session_id: String,
    pipeline: PipelineExecution,
    prompt: &'static str,
    _liveness: session_runner::LivenessHold,
}

/// Insert the session and pending-commit rows for a rebase/squash that runs now.
///
/// Callers must hold the branch launch lock: these rows are what make the branch
/// look busy to a concurrent launch, so the busy check and this insert have to be
/// atomic. Synchronous for the same reason — the lock must not be held across an
/// await.
fn insert_running_commit_pipeline_session(
    store: &Arc<Store>,
    registry: &Arc<session_runner::SessionRegistry>,
    ctx: &BranchPipelineContext,
    kind: PipelineKind,
    steps: &[PipelineStep],
    rebase_target: Option<String>,
    provider: Option<&str>,
) -> Result<RunningPipelineSession, String> {
    let prompt = pipeline_prompt(&kind, PushVariant::default());
    let mut pipeline = PipelineExecution::from_steps(steps).with_kind(kind);
    if let Some(target) = rebase_target {
        pipeline = pipeline.with_rebase_target(target);
    }

    let mut session =
        store::Session::new_running(prompt, &ctx.working_dir).with_branch(&ctx.branch.id);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    let liveness = registry.hold(&session.id);
    store.create_session(&session).map_err(|e| e.to_string())?;

    let commit = store::Commit::new_pending(&ctx.branch.id).with_session(&session.id);
    store.create_commit(&commit).map_err(|e| e.to_string())?;

    Ok(RunningPipelineSession {
        session_id: session.id,
        pipeline,
        prompt,
        _liveness: liveness,
    })
}

/// Announce a run-now pipeline session and hand it to the session runner.
///
/// Runs after the branch launch lock is released: the session row already exists,
/// so anything racing this already sees the branch as busy.
#[allow(clippy::too_many_arguments)]
fn launch_running_pipeline_session(
    ctx: BranchPipelineContext,
    running: RunningPipelineSession,
    steps: Vec<PipelineStep>,
    session_type: &str,
    provider: Option<String>,
    store: Arc<Store>,
    app_handle: &tauri::AppHandle,
    registry: &Arc<session_runner::SessionRegistry>,
) -> Result<String, String> {
    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

    session_runner::emit_session_running(
        app_handle,
        &running.session_id,
        &branch_id,
        &project_id,
        session_type,
    );

    let RunningPipelineSession {
        session_id,
        pipeline,
        prompt,
        _liveness: liveness,
    } = running;
    let started = session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: session_id.clone(),
            prompt: prompt.to_string(),
            steps,
            pipeline,
            working_dir: ctx.working_dir,
            pre_head_sha: None,
            provider,
            workspace_name: ctx.workspace_name,
            remote_working_dir: ctx.remote_working_dir,
            branch_id: Some(branch_id.clone()),
            project_id: Some(project_id.clone()),
        },
        Arc::clone(&store),
        app_handle.clone(),
        Arc::clone(registry),
    );
    if let Err(error) = started {
        // The row is `running` under our pid and the hold is still ours, so
        // the failure goes on the row *before* the hold is released: left
        // `running` with no owner, the sweep would end it within a minute as
        // "Staged lost track of this session" in place of the real cause.
        // The kick is the terminal-state drain a runner would have owed.
        if session_runner::finish_failed_before_run(
            &session_id,
            Some(branch_id.clone()),
            Some(project_id),
            &store,
            app_handle,
            &error,
        ) {
            crate::session_queue::kick(
                app_handle,
                &branch_id,
                crate::session_queue::KickSource::StartFailed,
            );
        }
        drop(liveness);
        return Err(error);
    }
    // The runner registered the session at entry; the row is its now.
    drop(liveness);

    Ok(session_id)
}

/// Queue a rebase/squash pipeline when the branch has work in flight.
///
/// Returns the queued session id — either a freshly created one, or an existing
/// queued pipeline that already covers this request — and `None` when the branch
/// is idle so the caller should start the pipeline immediately.
///
/// The busy check, the dedupe scan, and the insert all run under the branch
/// launch lock, so two rapid clicks (or a click racing a session start) cannot
/// both observe an idle branch or both miss the same queued pipeline. This stays
/// synchronous on purpose: the lock must not be held across an await.
///
/// With `then_force_push`, the force push that follows the rebase is queued in
/// the same critical section (see [`with_follow_up_force_push_locked`]).
fn queue_commit_pipeline_if_branch_busy(
    store: &Arc<Store>,
    branch_id: &str,
    kind: &PipelineKind,
    provider: Option<&str>,
    target: Option<&str>,
    then_force_push: bool,
) -> Result<Option<BranchPipelineResponse>, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();
    queue_commit_pipeline_locked(store, branch_id, kind, provider, target)?
        .map(|session_id| {
            with_follow_up_force_push_locked(
                store,
                branch_id,
                provider,
                BranchPipelineResponse::queued(session_id),
                then_force_push,
            )
        })
        .transpose()
}

/// Queue the force push of "Rebase and force push" behind `response`'s rebase,
/// gated on that rebase succeeding, and report it on the response.
///
/// Callers must hold the branch launch lock, the same hold that put the rebase
/// row in place. That is what guarantees the push row exists before the rebase
/// can finish: the drain that runs on the rebase's terminal state is the only
/// thing that releases (or skips) the push, so a push inserted after it ran
/// would sit in the queue with nothing left to drain it.
fn with_follow_up_force_push_locked(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    mut response: BranchPipelineResponse,
    then_force_push: bool,
) -> Result<BranchPipelineResponse, String> {
    if then_force_push {
        response.follow_up_session_id = Some(queue_force_push_after_locked(
            store,
            branch_id,
            provider,
            &response.session_id,
        )?);
    }
    Ok(response)
}

/// [`with_follow_up_force_push_locked`] for a rebase row already inserted as
/// `running`, with the failure path repaired: if the push cannot be queued,
/// the rebase is marked errored with the store error rather than left
/// `running`. Left as it was, the row would have no runner and no cancel
/// token, and would keep the branch busy until the startup sweep.
///
/// Failing the rebase is simpler than inserting the push first. The push row
/// is stamped from the rebase row — its id as the dependency and a
/// `created_at` strictly after it — so the inserts cannot be swapped without
/// restructuring the push path, and a push left behind by a failed rebase
/// insert would be a queued row gated on a session that never existed, which
/// keeps the branch busy with nothing to drain it. This runs under the same
/// launch lock and before the row is announced, so no client has seen the
/// rebase running when it is failed.
///
/// The failure goes through [`session_runner::finish_failed_before_run`], so
/// the `error` status event is emitted the way every other pre-run failure
/// emits it. The requesting client only toasts the returned error; without the
/// event, the pending commit stub the rebase insert created would show as
/// failed on every client only after the next unrelated timeline refresh.
/// Nothing is drained on this path: the branch was idle when the rebase row
/// went in, and the lock held since then kept it that way.
fn with_follow_up_force_push_or_fail_rebase_locked<R: tauri::Runtime>(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle<R>,
    branch_id: &str,
    project_id: &str,
    provider: Option<&str>,
    rebase_session_id: &str,
    then_force_push: bool,
) -> Result<BranchPipelineResponse, String> {
    let result = with_follow_up_force_push_locked(
        store,
        branch_id,
        provider,
        BranchPipelineResponse::running(rebase_session_id.to_string()),
        then_force_push,
    );
    if let Err(reason) = &result {
        // The row was inserted under this lock and never announced, so no
        // Stop can have taken it first; the transition is expected to win,
        // and there is no queue to drain whichever way it goes.
        let _ = session_runner::finish_failed_before_run(
            rebase_session_id,
            Some(branch_id.to_string()),
            Some(project_id.to_string()),
            store,
            app_handle,
            reason,
        );
    }
    result
}

/// The body of [`queue_commit_pipeline_if_branch_busy`], for the run-now path,
/// which re-checks while already holding the branch launch lock.
fn queue_commit_pipeline_locked(
    store: &Arc<Store>,
    branch_id: &str,
    kind: &PipelineKind,
    provider: Option<&str>,
    target: Option<&str>,
) -> Result<Option<String>, String> {
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;
    if !branch_has_work_in_flight(store, branch_id)? {
        return Ok(None);
    }

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let base_branch = base_branch_name(&branch);
    // A queued "rebase onto origin" marks its target with the branch's own
    // local name. The branch on origin is resolved — workspace-aware, and
    // without a git subprocess under this lock — when the row is dequeued,
    // the same way a queued push or pull is rebuilt.
    let rebase_ref = rebase_ref_for_target(&branch, &branch.branch_name, target);
    let rebase_target = persisted_rebase_target(target, &rebase_ref);

    if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
        pipeline.kind.as_ref() == Some(kind)
            && pipeline.rebase_target.as_deref() == rebase_target.as_deref()
    })? {
        return Ok(Some(existing));
    }

    let steps = build_commit_pipeline_steps(kind, base_branch, &rebase_ref)?;
    let mut pipeline = PipelineExecution::from_steps(&steps).with_kind(kind.clone());
    if let Some(target) = rebase_target {
        pipeline = pipeline.with_rebase_target(target);
    }
    let mut session = store::Session::new_queued(pipeline_prompt(kind, PushVariant::default()))
        .with_branch(branch_id);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline);
    store.create_session(&session).map_err(|e| e.to_string())?;

    let commit = store::Commit::new_pending(branch_id).with_session(&session.id);
    store.create_commit(&commit).map_err(|e| e.to_string())?;

    Ok(Some(session.id))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_or_queue_commit_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    kind: PipelineKind,
    provider: Option<String>,
    target: Option<String>,
    then_force_push: bool,
) -> Result<BranchPipelineResponse, String> {
    if then_force_push && kind != PipelineKind::Rebase {
        return Err(format!("{kind:?} cannot be followed by a force push"));
    }

    // Pre-flight check: a branch that is already busy queues without resolving a
    // pipeline context, which for a remote branch can depend on a running
    // workspace the queued work will only need later.
    if let Some(response) = queue_commit_pipeline_if_branch_busy(
        &store,
        &branch_id,
        &kind,
        provider.as_deref(),
        target.as_deref(),
        then_force_push,
    )? {
        return Ok(response);
    }

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let base_branch = base_branch_name(&ctx.branch).to_string();
    let remote_branch = crate::branches::resolve_remote_branch(&store, &ctx.branch).await;
    let rebase_ref = rebase_ref_for_target(&ctx.branch, &remote_branch, target.as_deref());
    let steps = build_commit_pipeline_steps(&kind, &base_branch, &rebase_ref)?;
    let rebase_target = persisted_rebase_target(target.as_deref(), &rebase_ref);

    // Re-check and insert under the lock, mirroring
    // `session_commands::start_or_queue_branch_session_for_store`: the pre-flight
    // check released the lock to resolve the context above, so without a second
    // look two near-simultaneous actions could both have seen an idle branch and
    // both start running — exactly what git-pipeline exclusivity exists to stop.
    let (running, response) = {
        let launch_lock = crate::session_commands::branch_session_launch_lock_for(&branch_id);
        let _guard = launch_lock.lock().unwrap();

        if let Some(session_id) = queue_commit_pipeline_locked(
            &store,
            &branch_id,
            &kind,
            provider.as_deref(),
            target.as_deref(),
        )? {
            return with_follow_up_force_push_locked(
                &store,
                &branch_id,
                provider.as_deref(),
                BranchPipelineResponse::queued(session_id),
                then_force_push,
            );
        }

        let running = insert_running_commit_pipeline_session(
            &store,
            &registry,
            &ctx,
            kind,
            &steps,
            rebase_target,
            provider.as_deref(),
        )?;
        // Still under the lock and before the runner has the rebase, so the
        // push is queued before the rebase can possibly finish.
        let response = with_follow_up_force_push_or_fail_rebase_locked(
            &store,
            &app_handle,
            &branch_id,
            &ctx.branch.project_id,
            provider.as_deref(),
            &running.session_id,
            then_force_push,
        )?;
        (running, response)
    };

    launch_running_pipeline_session(
        ctx,
        running,
        steps,
        "commit",
        provider,
        store,
        &app_handle,
        &registry,
    )?;

    Ok(response)
}

/// Start a queued rebase or squash that the branch queue (or a forced start)
/// has decided may run.
///
/// A persisted `rebase_target` only says the rebase is "onto origin"; the
/// branch on origin is resolved here, not replayed from the queued row, so a
/// Blox branch (whose queue path never asks its workspace) and a branch
/// renamed while it waited both rebase onto the branch they actually push to.
///
/// `claim` is what actually takes the row: it re-checks the branch under the
/// launch lock and flips the row to `running` in one step, so the decision the
/// caller made before its awaits cannot be overtaken by another start.
pub(crate) async fn start_queued_commit_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    session: store::Session,
    provider: Option<String>,
    claim: QueuedSessionClaim,
) -> Result<QueuedSessionStart, String> {
    let kind = session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.kind.clone())
        .ok_or_else(|| format!("Queued session {} has no pipeline kind", session.id))?;
    let target = session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.rebase_target.as_deref())
        .map(|_| "origin");

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let base_branch = base_branch_name(&ctx.branch).to_string();
    let remote_branch = if target.is_some() {
        crate::branches::resolve_remote_branch(&store, &ctx.branch).await
    } else {
        ctx.branch.branch_name.clone()
    };
    let rebase_ref = rebase_ref_for_target(&ctx.branch, &remote_branch, target);
    let rebase_target = persisted_rebase_target(target, &rebase_ref);
    let steps = build_commit_pipeline_steps(&kind, &base_branch, &rebase_ref)?;
    let prompt = pipeline_prompt(&kind, PushVariant::default());
    let mut pipeline = PipelineExecution::from_steps(&steps).with_kind(kind);
    if let Some(target) = rebase_target {
        pipeline = pipeline.with_rebase_target(target);
    }
    let effective_provider = session.provider.clone().or(provider);

    // Live from before the claim flips the row to `running` until
    // `start_pipeline_session` has registered it (see
    // `SessionRegistry::hold`); released when this function returns.
    let _liveness = registry.hold(&session.id);

    let claimed = claim.claim(&store)?;
    if claimed != QueuedSessionStart::Started {
        return Ok(claimed);
    }

    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

    // The row is ours and `running` from here: a failure before the handoff
    // ends it itself (see `fail_claimed_queued_start`).
    let prepared = (|| -> Result<(), String> {
        store
            .mark_session_artifact_started(&session.id)
            .map_err(|e| e.to_string())?;

        store
            .prepare_queued_session(&session.id, &ctx.working_dir.to_string_lossy(), prompt)
            .map_err(|e| e.to_string())?;
        store
            .update_session_pipeline(&session.id, &pipeline)
            .map_err(|e| e.to_string())
    })();
    if let Err(error) = prepared {
        return Err(fail_claimed_queued_start(
            &store,
            &app_handle,
            &branch_id,
            &project_id,
            &session.id,
            error,
        ));
    }

    session_runner::emit_session_running(
        &app_handle,
        &session.id,
        &branch_id,
        &project_id,
        "commit",
    );

    session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: session.id.clone(),
            prompt: prompt.to_string(),
            steps,
            pipeline,
            working_dir: ctx.working_dir,
            pre_head_sha: None,
            provider: effective_provider,
            workspace_name: ctx.workspace_name,
            remote_working_dir: ctx.remote_working_dir,
            branch_id: Some(branch_id),
            project_id: Some(project_id),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )
    .map(QueuedSessionStart::from_runner)
}

/// Insert the session row for a push that runs right now.
///
/// Unlike [`start_pipeline_for_branch`], the session records its pipeline kind
/// and `branch_id`. A push creates no artifact, so without that link the branch
/// queue could not see it and a commit session could start mid-push.
///
/// Like [`insert_running_commit_pipeline_session`], callers must hold the branch
/// launch lock: this row is what makes the branch look busy.
fn insert_running_push_pipeline_session(
    store: &Arc<Store>,
    registry: &Arc<session_runner::SessionRegistry>,
    ctx: &BranchPipelineContext,
    variant: PushVariant,
    steps: &[PipelineStep],
    provider: Option<&str>,
) -> Result<RunningPipelineSession, String> {
    let prompt = pipeline_prompt(&PipelineKind::Push, variant);
    let pipeline = PipelineExecution::from_steps(steps)
        .with_kind(PipelineKind::Push)
        .with_push_force(variant.force)
        .with_push_no_verify(variant.no_verify);

    let mut session =
        store::Session::new_running(prompt, &ctx.working_dir).with_branch(&ctx.branch.id);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    let liveness = registry.hold(&session.id);
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(RunningPipelineSession {
        session_id: session.id,
        pipeline,
        prompt,
        _liveness: liveness,
    })
}

/// Queue a push pipeline when the branch has work in flight.
///
/// Returns the queued session id — either a freshly created one, or an existing
/// queued push that already covers this request — and `None` when the branch is
/// idle so the caller should push immediately.
///
/// Mirrors [`queue_commit_pipeline_if_branch_busy`]: the busy check, the dedupe
/// scan, and the insert all run under the branch launch lock, and stay
/// synchronous because the lock must not be held across an await. Each
/// [`PushVariant`] dedupes separately — they are different operations, so a
/// queued normal push must not swallow a force push request, and a push that
/// runs the `pre-push` hook must not swallow one that skips it (or the
/// reverse).
fn queue_push_pipeline_if_branch_busy(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    variant: PushVariant,
) -> Result<Option<String>, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();
    queue_push_pipeline_locked(store, branch_id, provider, variant)
}

/// The body of [`queue_push_pipeline_if_branch_busy`], for the run-now path,
/// which re-checks while already holding the branch launch lock.
fn queue_push_pipeline_locked(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    variant: PushVariant,
) -> Result<Option<String>, String> {
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;
    if !branch_has_work_in_flight(store, branch_id)? {
        return Ok(None);
    }

    // A push gated on a rebase is a different request from an unconditional
    // one: folding a plain push into it would make the user's push depend on a
    // rebase they didn't ask about.
    if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
        pipeline.kind.as_ref() == Some(&PipelineKind::Push)
            && PushVariant::of(pipeline) == variant
            && pipeline.depends_on_session_id.is_none()
    })? {
        return Ok(Some(existing));
    }

    insert_queued_push_session(store, branch_id, provider, variant, None).map(Some)
}

/// The push "Rebase and force push" queues behind its rebase. It always runs
/// the `pre-push` hook: the menu offers no variant that skips it.
const FOLLOW_UP_FORCE_PUSH: PushVariant = PushVariant {
    force: true,
    no_verify: false,
};

/// Queue the force push that runs after `dependency_id` — the rebase of a
/// "Rebase and force push" — and only if that rebase succeeds.
///
/// Always queues, even on an idle branch: the push must not start before the
/// rebase has run, and the drain that follows the rebase is what decides
/// whether it starts at all (see `pipeline_dependency`). A repeat request for
/// the same rebase dedupes onto the push already waiting on it.
///
/// Callers must hold the branch launch lock.
fn queue_force_push_after_locked(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    dependency_id: &str,
) -> Result<String, String> {
    if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
        pipeline.kind.as_ref() == Some(&PipelineKind::Push)
            && PushVariant::of(pipeline) == FOLLOW_UP_FORCE_PUSH
            && pipeline.depends_on_session_id.as_deref() == Some(dependency_id)
    })? {
        return Ok(existing);
    }

    insert_queued_push_session(
        store,
        branch_id,
        provider,
        FOLLOW_UP_FORCE_PUSH,
        Some(dependency_id),
    )
}

/// Insert a queued push row, optionally gated on another session.
fn insert_queued_push_session(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    variant: PushVariant,
    depends_on: Option<&str>,
) -> Result<String, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    // A queue marker only: the steps are rebuilt with a full resolution on
    // dequeue, so the branch on origin is not looked up here — that would be
    // a git subprocess under the branch launch lock.
    let steps = build_push_pipeline_steps(&branch.branch_name, &branch.branch_name, variant);
    let pipeline = PipelineExecution::from_steps(&steps)
        .with_kind(PipelineKind::Push)
        .with_push_force(variant.force)
        .with_push_no_verify(variant.no_verify)
        .with_depends_on_session_id(depends_on.map(str::to_string));
    // No artifact row: a push produces no commit, and a pending-commit stub
    // would render as a failed commit once the push finishes without a new sha.
    // `branch_id` is what keeps this session on the branch queue instead.
    let mut session = store::Session::new_queued(pipeline_prompt(&PipelineKind::Push, variant))
        .with_branch(branch_id);
    if let Some(dependency_id) = depends_on {
        // Sort strictly after the dependency. The two rows are created in the
        // same critical section, so they can share a millisecond, and
        // `created_at` is what orders both the branch queue and the timeline.
        if let Some(dependency) = store
            .get_session(dependency_id)
            .map_err(|e| e.to_string())?
        {
            if session.created_at <= dependency.created_at {
                session.created_at = dependency.created_at + 1;
                session.updated_at = session.created_at;
            }
        }
    }
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline);
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(session.id)
}

/// A queued push or pull, rebuilt for the run that is about to start.
struct RebuiltGitPipeline {
    steps: Vec<PipelineStep>,
    pipeline: PipelineExecution,
    prompt: &'static str,
    session_type: &'static str,
}

/// Rebuild a queued push or pull from its persisted flags and the branch's
/// current names.
///
/// The [`PushVariant`] carries over and is persisted again on the running
/// pipeline, so a no-verify push that waited in the queue still skips the
/// `pre-push` hook. `depends_on_session_id` carries over too, so the push still
/// records what it waited on.
fn rebuild_queued_git_pipeline(
    session_id: &str,
    queued: &PipelineExecution,
    local_branch: &str,
    remote_branch: &str,
) -> Result<RebuiltGitPipeline, String> {
    let kind = queued
        .kind
        .clone()
        .ok_or_else(|| format!("Queued session {session_id} has no pipeline kind"))?;
    let variant = PushVariant::of(queued);
    let (steps, session_type) = match kind {
        PipelineKind::Push => (
            build_push_pipeline_steps(local_branch, remote_branch, variant),
            "push",
        ),
        PipelineKind::Pull => (build_pull_pipeline_steps(remote_branch), "pull"),
        PipelineKind::Rebase | PipelineKind::Squash => {
            return Err(format!(
                "Queued git pipeline session {session_id} has non-git kind {kind:?}"
            ));
        }
    };
    let prompt = pipeline_prompt(&kind, variant);
    let pipeline = PipelineExecution::from_steps(&steps)
        .with_kind(kind)
        .with_push_force(variant.force)
        .with_push_no_verify(variant.no_verify)
        .with_depends_on_session_id(queued.depends_on_session_id.clone());
    Ok(RebuiltGitPipeline {
        steps,
        pipeline,
        prompt,
        session_type,
    })
}

/// Start a queued push or pull pipeline that reached the front of the branch
/// queue.
///
/// Steps are rebuilt from the branch's current name and the persisted push
/// flags rather than replayed from the queued pipeline (see
/// [`rebuild_queued_git_pipeline`]), matching
/// [`start_queued_commit_pipeline_for_branch`] so a branch renamed while the work
/// waited still acts on the right ref. `claim` takes the row the same way too.
pub(crate) async fn start_queued_git_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    session: store::Session,
    provider: Option<String>,
    claim: QueuedSessionClaim,
) -> Result<QueuedSessionStart, String> {
    let queued_pipeline = session
        .pipeline
        .as_ref()
        .ok_or_else(|| format!("Queued session {} has no pipeline", session.id))?;

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let remote_branch = crate::branches::resolve_remote_branch(&store, &ctx.branch).await;
    let RebuiltGitPipeline {
        steps,
        pipeline,
        prompt,
        session_type,
    } = rebuild_queued_git_pipeline(
        &session.id,
        queued_pipeline,
        &ctx.branch.branch_name,
        &remote_branch,
    )?;
    let effective_provider = session.provider.clone().or(provider);

    // Live from before the claim flips the row to `running` until
    // `start_pipeline_session` has registered it (see
    // `SessionRegistry::hold`); released when this function returns.
    let _liveness = registry.hold(&session.id);

    let claimed = claim.claim(&store)?;
    if claimed != QueuedSessionStart::Started {
        return Ok(claimed);
    }

    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

    // No `mark_session_artifact_started` call: a git pipeline has no queued
    // artifact stub whose timestamp needs restamping when the work actually
    // starts.
    //
    // The row is ours and `running` from here: a failure before the handoff
    // ends it itself (see `fail_claimed_queued_start`).
    let prepared = (|| -> Result<(), String> {
        store
            .prepare_queued_session(&session.id, &ctx.working_dir.to_string_lossy(), prompt)
            .map_err(|e| e.to_string())?;
        store
            .update_session_pipeline(&session.id, &pipeline)
            .map_err(|e| e.to_string())
    })();
    if let Err(error) = prepared {
        return Err(fail_claimed_queued_start(
            &store,
            &app_handle,
            &branch_id,
            &project_id,
            &session.id,
            error,
        ));
    }

    session_runner::emit_session_running(
        &app_handle,
        &session.id,
        &branch_id,
        &project_id,
        session_type,
    );

    session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: session.id.clone(),
            prompt: prompt.to_string(),
            steps,
            pipeline,
            working_dir: ctx.working_dir,
            pre_head_sha: None,
            provider: effective_provider,
            workspace_name: ctx.workspace_name,
            remote_working_dir: ctx.remote_working_dir,
            branch_id: Some(branch_id),
            project_id: Some(project_id),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )
    .map(QueuedSessionStart::from_runner)
}

/// What the branch queue decided to do with a pull request.
enum PullDisposition {
    /// Waiting its turn: either a freshly queued session, or the queued pull that
    /// already covers this request.
    Queued(String),
    /// The branch was idle, so the pull runs now. The id is the session that marks
    /// the branch busy for its duration, and the hold is what says this process
    /// will finish it (see `SessionRegistry::hold`) — it never goes through the
    /// runner, so nothing else registers it.
    RunningNow {
        session_id: String,
        liveness: session_runner::LivenessHold,
    },
}

/// Decide between pulling now and queueing behind in-flight branch work, and
/// record that decision.
///
/// Mirrors [`queue_push_pipeline_if_branch_busy`]: the busy check, the dedupe
/// scan, and the insert all run under the branch launch lock, and stay
/// synchronous because the lock must not be held across an await. A pull has no
/// variants, so the dedupe keys on the kind alone.
///
/// The run-now case still gets a session row — a running `PipelineKind::Pull`
/// linked to the branch with no artifact, exactly like a running push. It is what
/// makes the fetch-and-merge visible to `has_running_session_for_branch` and the
/// drain scan; without it the one mutating git operation that skips the pipeline
/// runner would look idle for its whole duration, and a commit session or another
/// git action could start against the same worktree mid-merge. No
/// `session-status-changed` event is emitted for it: the caller awaits the pull and
/// reports the outcome itself, so an event would double-report a failure and spin
/// the project tile for what is usually an instant operation.
///
/// No provider is recorded either: every pull step aborts on failure, so the
/// pipeline never hands off to an agent.
fn claim_or_queue_pull_for_branch(
    store: &Arc<Store>,
    registry: &Arc<session_runner::SessionRegistry>,
    branch_id: &str,
) -> Result<PullDisposition, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;

    let busy = branch_has_work_in_flight(store, branch_id)?;
    if busy {
        if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
            pipeline.kind.as_ref() == Some(&PipelineKind::Pull)
        })? {
            return Ok(PullDisposition::Queued(existing));
        }
    }

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    // A queued pull is rebuilt with a full resolution on dequeue, and a pull
    // that runs now goes through `pull_branch_ff_only_impl`, which resolves
    // the remote branch itself; these steps are the queue marker, so the
    // branch on origin is not looked up under the launch lock here.
    let steps = build_pull_pipeline_steps(&branch.branch_name);
    let pipeline = PipelineExecution::from_steps(&steps).with_kind(PipelineKind::Pull);
    // Like a push session, this one carries no artifact — `branch_id` is what
    // keeps it on the branch queue.
    let mut session = if busy {
        store::Session::new_queued(PULL_PROMPT)
    } else {
        store::Session::new_running(PULL_PROMPT, &immediate_pull_working_dir(store, branch_id))
    }
    .with_branch(branch_id);
    session.pipeline = Some(pipeline);
    // Before the write, so the row is never `running` without it.
    let liveness = (!busy).then(|| registry.hold(&session.id));
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(match liveness {
        None => PullDisposition::Queued(session.id),
        Some(liveness) => PullDisposition::RunningNow {
            session_id: session.id,
            liveness,
        },
    })
}

/// Best-effort working directory for the session row of an immediate pull.
///
/// The pull resolves its own path (a remote branch fast-forwards through its
/// workspace shell), so this only decides what the session row displays — not
/// where anything runs, and not whether the pull can proceed.
fn immediate_pull_working_dir(store: &Arc<Store>, branch_id: &str) -> PathBuf {
    store
        .get_workdir_for_branch(branch_id)
        .ok()
        .flatten()
        .map(|workdir| PathBuf::from(workdir.path))
        .unwrap_or_default()
}

/// Release the session that marked the branch busy for an immediate pull.
///
/// The marker never went through the session runner, so nothing else will end it.
/// The completion reason matches the pipeline path: the work ran to its conclusion
/// either way, only the outcome differs.
fn finish_immediate_pull_session(store: &Arc<Store>, session_id: &str, error: Option<&str>) {
    let status = if error.is_some() {
        store::SessionStatus::Error
    } else {
        store::SessionStatus::Completed
    };

    if let Err(e) = store.transition_from_running(
        session_id,
        status,
        error,
        Some(&store::CompletionReason::TurnComplete),
    ) {
        log::warn!("[prs] Failed to finish immediate pull session {session_id}: {e}");
    }
}

/// Fast-forward the branch to origin now, or queue the pull behind in-flight
/// branch work.
///
/// An idle branch pulls directly rather than through the pipeline runner, which is
/// why this returns an `Option` rather than the [`BranchPipelineResponse`] the
/// pipeline-only actions use: `None` means the pull already happened (and its
/// failure, if any, is this call's error), `Some(session_id)` means it is waiting
/// its turn on the branch queue.
pub(crate) async fn pull_or_queue_branch_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<Option<String>, String> {
    let pull_store = Arc::clone(&store);
    let pull_branch_id = branch_id.clone();
    let kick_branch_id = branch_id.clone();
    pull_or_queue_branch_with(
        store,
        registry,
        branch_id,
        move || crate::timeline::pull_branch_ff_only_impl(&pull_store, &pull_branch_id),
        move || {
            // Anything the user requested while the pull held the branch is
            // queued behind the marker session, and the marker bypassed the
            // session runner that would normally drain it.
            crate::session_queue::kick(
                &app_handle,
                &kick_branch_id,
                crate::session_queue::KickSource::ImmediatePull,
            );
        },
    )
    .await
}

/// [`pull_or_queue_branch_for_branch`] with the pull itself and the
/// after-release hook injected, so a test can stand in a pull that blocks.
async fn pull_or_queue_branch_with(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    branch_id: String,
    pull: impl FnOnce() -> Result<(), String> + Send + 'static,
    on_released: impl FnOnce() + Send + 'static,
) -> Result<Option<String>, String> {
    let (session_id, liveness) =
        match claim_or_queue_pull_for_branch(&store, &registry, &branch_id)? {
            PullDisposition::Queued(session_id) => return Ok(Some(session_id)),
            PullDisposition::RunningNow {
                session_id,
                liveness,
            } => (session_id, liveness),
        };

    // From here on the marker row is the branch's busy state, and this future
    // belongs to whoever asked for the pull: in web mode that is a request
    // handler hyper drops at its next `.await` the moment the client closes
    // the connection. The work that releases the marker must therefore run on
    // its own task, with this future only waiting on the handle — dropping a
    // `JoinHandle` detaches the task, it does not cancel it.
    //
    // A join failure (the pull task panicking, or runtime shutdown) folds into
    // the pull's own error path rather than returning early: the marker
    // session is what marks the branch busy, and nothing else ever finishes it.
    let released = tauri::async_runtime::spawn(async move {
        let pulled = tauri::async_runtime::spawn_blocking(pull)
            .await
            .unwrap_or_else(|e| Err(format!("Pull task failed: {e}")));

        finish_immediate_pull_session(
            &store,
            &session_id,
            pulled.as_ref().err().map(String::as_str),
        );
        // Released after the terminal write, on the task that owns the marker
        // — not with the request future, which may already be gone.
        drop(liveness);
        on_released();

        pulled
    });

    released
        .await
        .unwrap_or_else(|e| Err(format!("Pull task failed: {e}")))
        .map(|()| None)
}

/// Pull origin's new commits into the branch.
///
/// Returns the queued session id when the pull had to join the branch queue, and
/// `None` when it ran immediately.
#[tauri::command(rename_all = "camelCase")]
pub async fn pull_or_queue_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<Option<String>, String> {
    let store = get_store(&store)?;
    pull_or_queue_branch_for_branch(store, Arc::clone(&registry), app_handle, branch_id).await
}

fn create_pr_handoff_prompt(
    pr_type: &str,
    base_branch: &str,
    base_ref: &str,
    head_branch: &str,
    draft_flag: &str,
    opening: &str,
) -> String {
    // Double braces survive format!() as the runtime template placeholder
    // replaced by run_pipeline().
    format!(
        r#"<action>
{opening}

Use the context below as a starting point, and inspect the branch changes as needed.

Requirements:
- Treat `{base_ref}` as the comparison base. The local `{base_branch}` branch may be stale; do not use it for diff or log comparisons.
- If you inspect branch changes, compare `$(git merge-base {base_ref} HEAD)..HEAD`.
- Choose a conventional-commit-style PR title, using the most appropriate type (feat, fix, refactor, docs, style, test, chore, perf, ci, or build) based on the actual changes.
- Write a concise PR body that summarizes the changes.
- Run `gh pr create --base {base_branch} --head {head_branch} --title <title> --body <body>{draft_flag}` to create the {pr_type}. `{head_branch}` is the branch on origin the previous step pushed to; it may differ from the local branch name.
- Do not use `--fill-first`.
- If `gh pr create` reports that a PR already exists, run `gh pr view --json url --jq .url`, update the title/body with `gh pr edit` if needed, and continue.
- When done, output exactly `PR_URL: <url>`.

Context from prior steps:

{{step_outputs}}
</action>"#
    )
}

fn build_create_pr_pipeline_steps(
    pr_type: &str,
    base_branch: &str,
    draft_flag: &str,
    local_branch: &str,
    remote_branch: &str,
) -> Vec<PipelineStep> {
    let base_ref = git::origin_ref_for_branch(base_branch);
    let refspec = push_refspec(local_branch, remote_branch);

    vec![
        PipelineStep::Command {
            label: "Fetch latest base".to_string(),
            command: git_fetch_with_fallback(base_branch),
            on_failure: FailureStrategy::HandoffToAi {
                prompt_template: create_pr_handoff_prompt(
                    pr_type,
                    base_branch,
                    &base_ref,
                    remote_branch,
                    draft_flag,
                    &format!(
                        "The fetch failed while creating a {pr_type}. Diagnose and fix the issue if needed, then inspect this branch against `{base_ref}`, push the branch, and create or recover the {pr_type}."
                    ),
                ),
            },
        },
        PipelineStep::Command {
            label: "View commit history".to_string(),
            command: format!(
                r#"base_commit=$(git merge-base {base_ref} HEAD) && git log --oneline "$base_commit"..HEAD"#
            ),
            on_failure: FailureStrategy::Continue,
        },
        PipelineStep::Command {
            label: "View changed files".to_string(),
            command: format!(
                r#"base_commit=$(git merge-base {base_ref} HEAD) && git diff "$base_commit"..HEAD --stat"#
            ),
            on_failure: FailureStrategy::Continue,
        },
        PipelineStep::Command {
            label: "Push to remote".to_string(),
            command: git_push_with_fallback(&format!("-u origin {refspec}")),
            on_failure: FailureStrategy::HandoffToAi {
                prompt_template: create_pr_handoff_prompt(
                    pr_type,
                    base_branch,
                    &base_ref,
                    remote_branch,
                    draft_flag,
                    &format!(
                        "The push failed while creating a {pr_type}. Diagnose and fix the issue, then retry the push with exactly `git push -u origin {refspec}`; do not push `{local_branch}` under any other remote name. After the push succeeds, create or recover the {pr_type}."
                    ),
                ),
            },
        },
        PipelineStep::AiHandoff {
            label: "Create PR".to_string(),
            prompt_template: create_pr_handoff_prompt(
                pr_type,
                base_branch,
                &base_ref,
                remote_branch,
                draft_flag,
                &format!("Create a {pr_type} for the current branch."),
            ),
        },
    ]
}

// =============================================================================
// Pipeline commands
// =============================================================================

/// Create a pull request for a branch by kicking off an agent session.
pub(crate) async fn start_create_pr_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    draft: Option<bool>,
) -> Result<String, String> {
    crate::project_transfer::ensure_branch_not_transferring(&branch_id)?;
    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let base_branch = base_branch_name(&ctx.branch);

    let is_draft = draft.unwrap_or(false);
    let draft_flag = if is_draft { " --draft" } else { "" };
    let pr_type = if is_draft {
        "draft pull request"
    } else {
        "pull request"
    };

    // Build the pipeline steps for PR creation.
    let remote_branch = crate::branches::resolve_remote_branch(&store, &ctx.branch).await;
    let steps = build_create_pr_pipeline_steps(
        pr_type,
        base_branch,
        draft_flag,
        &ctx.branch.branch_name,
        &remote_branch,
    );

    let prompt = format!("Create a {pr_type} for the current branch");

    start_pipeline_for_branch(
        ctx,
        steps,
        &prompt,
        "pr",
        provider,
        store,
        &app_handle,
        &registry,
    )
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    draft: Option<bool>,
) -> Result<String, String> {
    let store = get_store(&store)?;
    start_create_pr_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
        draft,
    )
    .await
}

/// Build the GitHub PR URL for a branch.
///
/// For fork PRs the stored repo may be the fork (head) repo, but PRs always
/// live on the base (upstream) repo. This queries the GitHub API to resolve
/// the canonical URL, falling back to the parent repo when the stored repo
/// is a fork.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_pr_url(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: u64,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let (repo_slug, _) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;

    tauri::async_runtime::spawn_blocking(move || git::fetch_pr_url(&repo_slug, pr_number))
        .await
        .map_err(|e| format!("get_pr_url task failed: {e}"))?
        .map_err(|e| e.to_string())
}

/// Update the PR number for a branch.
#[tauri::command(rename_all = "camelCase")]
pub fn update_branch_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: Option<u64>,
) -> Result<(), String> {
    get_store(&store)?
        .update_branch_pr_number(&branch_id, pr_number, None)
        .map_err(|e| e.to_string())
}

/// Refresh PR status for a single branch.
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_pr_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    refresh_pr_status_impl(store, app_handle, branch_id).await
}

/// Core implementation of the single-branch PR status refresh, shared by the
/// `refresh_pr_status` command, the web-mode `dispatch()` arm, and the
/// session-completion side effects that run after a PR session finishes.
pub(crate) async fn refresh_pr_status_impl(
    store: Arc<Store>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let pr_number = branch
        .pr_number
        .ok_or_else(|| "Branch does not have an associated PR".to_string())?;
    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
    let (github_repo, _) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;

    // Run the blocking gh CLI call on a background thread so we don't
    // block the Tauri IPC thread and starve other commands.
    let pr_status = {
        let github_repo = github_repo.clone();
        tauri::async_runtime::spawn_blocking(move || {
            git::fetch_pr_status_for_repo(&github_repo, pr_number)
        })
        .await
        .map_err(|e| format!("refresh_pr_status task failed: {e}"))?
    };
    let pr_status = match pr_status {
        Ok(status) => status,
        Err(e) => {
            log::error!(
                "refresh_pr_status failed for branch_id={}, pr_number={}: {}",
                branch_id,
                pr_number,
                e
            );
            return Err(e.to_string());
        }
    };
    let mergeable = pr_status.mergeable == "MERGEABLE";
    let pr_fetched_at = store::now_timestamp();

    let written = store
        .update_branch_pr_status(
            &branch_id,
            Some(pr_number),
            Some(pr_status.state.clone()),
            Some(pr_status.checks_summary.state.clone()),
            pr_status.review_decision.clone(),
            Some(mergeable),
            Some(pr_status.is_draft),
            None,
            None,
            pr_status.head_sha.clone(),
            pr_status.title.clone(),
        )
        .map_err(|e| e.to_string())?;
    if !written {
        // Relinked or unlinked during the fetch: this status is another PR's.
        log::debug!("refresh_pr_status: branch_id={branch_id} no longer links PR #{pr_number}");
        return Ok(());
    }

    crate::web_server::emit_to_all(
        &app_handle,
        "pr-status-changed",
        PrStatusEvent {
            branch_id: branch_id.clone(),
            pr_state: pr_status.state,
            pr_checks_status: pr_status.checks_summary.state,
            pr_review_decision: pr_status.review_decision,
            pr_mergeable: mergeable,
            pr_draft: pr_status.is_draft,
            pr_head_sha: pr_status.head_sha,
            pr_fetched_at,
            failed_checks: pr_status.failed_checks,
        },
    );

    Ok(())
}

/// Max concurrent PR-status fetches inside a single project refresh. Each fetch
/// spawns a `gh` subprocess + GitHub round-trip, so we cap the fan-out to avoid
/// a subprocess thundering herd while still resolving a project's PRs in ~1
/// round-trip's wall-clock instead of N (fully serial). The backend PR-poll
/// scheduler reuses this cap for a single pool shared across all the projects
/// it refreshes (see `pr_poll_scheduler`).
pub(crate) const PR_REFRESH_CONCURRENCY: usize = 6;

/// Refresh PR status for all branches in a project.
///
/// Thin command wrapper around [`refresh_project_pr_statuses`]; the same core is
/// also driven on a cadence by the backend PR-poll scheduler. Each command call
/// gets its own bounded pool (the scheduler instead shares one pool across
/// projects).
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_all_pr_statuses(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    project_id: String,
) -> Result<u32, String> {
    let store = get_store(&store)?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(PR_REFRESH_CONCURRENCY));
    refresh_project_pr_statuses(&store, &app_handle, &project_id, semaphore).await
}

/// Core implementation shared by the `refresh_all_pr_statuses` command and the
/// backend PR-poll scheduler.
///
/// Fans the per-branch fetches out across the bounded `semaphore` pool instead
/// of awaiting them one at a time. Repo resolution is a cheap local DB read so
/// it stays on this task; only the network fetch + DB write + per-branch
/// `pr-status-changed` emit move into the spawned tasks, gated by the semaphore.
/// A final `pr-statuses-refreshed` event is emitted and the number of branches
/// refreshed is returned.
///
/// The semaphore is passed in (rather than created per call) so the scheduler
/// can share a single pool across every project it refreshes — a tick that
/// finds many projects due still caps total concurrent `gh` subprocesses.
pub(crate) async fn refresh_project_pr_statuses(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle,
    project_id: &str,
    semaphore: Arc<tokio::sync::Semaphore>,
) -> Result<u32, String> {
    let project = store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let branches = store
        .list_branches_for_project(project_id)
        .map_err(|e| e.to_string())?;
    let branches_with_prs: Vec<_> = branches
        .into_iter()
        .filter(|b| b.pr_number.is_some())
        .collect();

    let mut tasks = Vec::new();

    for branch in branches_with_prs {
        let pr_number = branch.pr_number.unwrap();
        let github_repo = match resolve_branch_repo_and_subpath(store, &project, &branch) {
            Ok((repo, _)) => repo,
            Err(e) => {
                log::warn!(
                    "Failed to resolve repo for branch {} (PR #{}): {}",
                    branch.id,
                    pr_number,
                    e
                );
                continue;
            }
        };

        let store = Arc::clone(store);
        let app_handle = app_handle.clone();
        let semaphore = Arc::clone(&semaphore);
        let branch_id = branch.id.clone();

        tasks.push(tauri::async_runtime::spawn(async move {
            // Hold a permit for the whole fetch so no more than
            // PR_REFRESH_CONCURRENCY `gh` round-trips are in flight at once.
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|e| format!("refresh_project_pr_statuses semaphore closed: {e}"))?;

            let pr_result = tauri::async_runtime::spawn_blocking(move || {
                git::fetch_pr_status_for_repo(&github_repo, pr_number)
            })
            .await
            .map_err(|e| format!("refresh_project_pr_statuses task failed: {e}"))?;

            match pr_result {
                Ok(pr_status) => {
                    let mergeable = pr_status.mergeable == "MERGEABLE";
                    let pr_fetched_at = store::now_timestamp();

                    match store.update_branch_pr_status(
                        &branch_id,
                        Some(pr_number),
                        Some(pr_status.state.clone()),
                        Some(pr_status.checks_summary.state.clone()),
                        pr_status.review_decision.clone(),
                        Some(mergeable),
                        Some(pr_status.is_draft),
                        None,
                        None,
                        pr_status.head_sha.clone(),
                        pr_status.title.clone(),
                    ) {
                        Ok(true) => {}
                        // Relinked or unlinked during the fetch: this status
                        // is another PR's, so don't emit it either. Nothing
                        // failed — the relink already wrote newer state.
                        Ok(false) => return Ok(BranchRefresh::Skipped),
                        Err(e) => {
                            log::warn!(
                                "Failed to update PR status for branch {}: {}",
                                branch_id,
                                e
                            );
                            return Ok(BranchRefresh::Failed);
                        }
                    }

                    crate::web_server::emit_to_all(
                        &app_handle,
                        "pr-status-changed",
                        PrStatusEvent {
                            branch_id: branch_id.clone(),
                            pr_state: pr_status.state,
                            pr_checks_status: pr_status.checks_summary.state,
                            pr_review_decision: pr_status.review_decision,
                            pr_mergeable: mergeable,
                            pr_draft: pr_status.is_draft,
                            pr_head_sha: pr_status.head_sha,
                            pr_fetched_at,
                            failed_checks: pr_status.failed_checks,
                        },
                    );

                    Ok::<BranchRefresh, String>(BranchRefresh::Refreshed)
                }
                Err(e) => {
                    log::warn!(
                        "Failed to fetch PR status for branch {} (PR #{}): {}",
                        branch_id,
                        pr_number,
                        e
                    );
                    Ok(BranchRefresh::Failed)
                }
            }
        }));
    }

    let refreshed_count = collect_branch_refresh_results(tasks).await?;

    crate::web_server::emit_to_all(app_handle, "pr-statuses-refreshed", project_id);

    Ok(refreshed_count)
}

/// How one branch's PR status refresh in [`refresh_project_pr_statuses`]
/// ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchRefresh {
    /// Fetched, written and emitted.
    Refreshed,
    /// Fetched, but the branch was relinked or unlinked during the fetch, so
    /// the status belonged to a PR it no longer links and wasn't written.
    /// Neither refreshed nor failed, as in the web `refresh_all_pr_statuses`
    /// arm: a project whose only PR branch was relinked mid-poll hasn't had a
    /// failed poll.
    Skipped,
    /// The fetch or the write failed.
    Failed,
}

async fn collect_branch_refresh_results<T, E>(tasks: Vec<T>) -> Result<u32, String>
where
    T: std::future::Future<Output = Result<Result<BranchRefresh, String>, E>>,
    E: std::fmt::Display,
{
    let mut refreshed_count = 0u32;
    let mut failed_branch_count = 0u32;
    let mut task_errors = Vec::new();

    for task in tasks {
        match task.await {
            Ok(Ok(BranchRefresh::Refreshed)) => refreshed_count += 1,
            Ok(Ok(BranchRefresh::Skipped)) => {}
            Ok(Ok(BranchRefresh::Failed)) => failed_branch_count += 1,
            Ok(Err(e)) => {
                log::warn!("PR status refresh task failed: {e}");
                task_errors.push(e);
            }
            Err(e) => {
                let message = format!("PR status refresh task join failed: {e}");
                log::warn!("{message}");
                task_errors.push(message);
            }
        }
    }

    if refreshed_count == 0 && (failed_branch_count > 0 || !task_errors.is_empty()) {
        let mut errors = Vec::new();
        if failed_branch_count > 0 {
            errors.push(format!("{failed_branch_count} branch refreshes failed"));
        }
        errors.extend(task_errors);

        return Err(format!(
            "all PR status refresh tasks failed: {}",
            errors.join("; ")
        ));
    }

    Ok(refreshed_count)
}

/// Clear stale PR status fields for a branch (e.g. after a push invalidates them).
///
/// This nulls out checks, mergeable, review-decision, etc. in the DB and emits
/// a `pr-status-cleared` event so the frontend can drop the stale indicators
/// immediately instead of waiting for the next GitHub refresh.
#[tauri::command(rename_all = "camelCase")]
pub fn clear_branch_pr_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    clear_branch_pr_status_impl(&store, &app_handle, &branch_id)
}

/// Core implementation shared by the `clear_branch_pr_status` command, the
/// web-mode `dispatch()` arm, and the session-completion side effects that
/// run after a push session finishes.
pub(crate) fn clear_branch_pr_status_impl(
    store: &Store,
    app_handle: &tauri::AppHandle,
    branch_id: &str,
) -> Result<(), String> {
    store
        .update_branch_pr_status(
            branch_id, None, None, None, None, None, None, None, None, None, None,
        )
        .map_err(|e| e.to_string())?;

    crate::web_server::emit_to_all(app_handle, "pr-status-cleared", branch_id);

    Ok(())
}

/// Check if a branch has commits that haven't been pushed to the remote.
#[tauri::command(rename_all = "camelCase")]
pub async fn has_unpushed_commits(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<bool, String> {
    let store = get_store(&store)?;
    has_unpushed_commits_impl(store, branch_id).await
}

pub(crate) async fn has_unpushed_commits_impl(
    store: Arc<Store>,
    branch_id: String,
) -> Result<bool, String> {
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if branch.branch_type == store::BranchType::Remote {
        let workspace_name = branch
            .workspace_name
            .as_deref()
            .ok_or_else(|| format!("Branch has no workspace name: {branch_id}"))?
            .to_string();
        let repo_subpath = crate::branches::resolve_branch_workspace_subpath(&store, &branch)?;
        let local_branch = branch.branch_name.clone();
        let base_branch = branch.base_branch.clone();

        // Run the blocking SSH calls on a background thread so we don't
        // block the Tauri IPC thread and freeze the UI.
        let result = tauri::async_runtime::spawn_blocking(move || {
            let remote_branch = crate::branches::workspace_upstream_branch_name(
                &workspace_name,
                repo_subpath.as_deref(),
                &local_branch,
                &base_branch,
            );
            let remote_ref = format!("origin/{remote_branch}");
            // Remote tracking branch doesn't exist — all commits are unpushed
            if crate::branches::run_workspace_git(
                &workspace_name,
                repo_subpath.as_deref(),
                &["rev-parse", "--verify", &remote_ref],
            )
            .is_err()
            {
                return Ok(true);
            }
            let rev_range = format!("{remote_ref}..HEAD");
            let output = crate::branches::run_workspace_git(
                &workspace_name,
                repo_subpath.as_deref(),
                &["rev-list", &rev_range],
            )
            .map_err(|e| e.to_string())?;
            Ok(!output.trim().is_empty())
        })
        .await
        .map_err(|e| format!("has_unpushed_commits task failed: {e}"))?;
        return result;
    }

    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    // Run the blocking git subprocesses on a background thread so a slow cold
    // `git` invocation can't block the Tauri IPC thread and freeze the UI,
    // matching the remote path above.
    let path = workdir.path.clone();
    let local_branch = branch.branch_name.clone();
    let base_branch = branch.base_branch.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let worktree = Path::new(&path);
        let remote_branch = git::upstream_branch_name(worktree, &local_branch, &base_branch);
        git::has_unpushed_commits(worktree, &remote_branch).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("has_unpushed_commits task failed: {e}"))?
}

/// Push a branch to its remote by kicking off an agent session, or queue the
/// push behind whatever the branch is already doing.
pub(crate) async fn start_or_queue_push_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    variant: PushVariant,
) -> Result<BranchPipelineResponse, String> {
    // Pre-flight check, then a re-check at insert time — see
    // `start_or_queue_commit_pipeline_for_branch` for why both are needed.
    if let Some(session_id) =
        queue_push_pipeline_if_branch_busy(&store, &branch_id, provider.as_deref(), variant)?
    {
        return Ok(BranchPipelineResponse::queued(session_id));
    }

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let remote_branch = crate::branches::resolve_remote_branch(&store, &ctx.branch).await;
    let steps = build_push_pipeline_steps(&ctx.branch.branch_name, &remote_branch, variant);

    let running = {
        let launch_lock = crate::session_commands::branch_session_launch_lock_for(&branch_id);
        let _guard = launch_lock.lock().unwrap();

        if let Some(session_id) =
            queue_push_pipeline_locked(&store, &branch_id, provider.as_deref(), variant)?
        {
            return Ok(BranchPipelineResponse::queued(session_id));
        }

        insert_running_push_pipeline_session(
            &store,
            &registry,
            &ctx,
            variant,
            &steps,
            provider.as_deref(),
        )?
    };

    let session_id = launch_running_pipeline_session(
        ctx,
        running,
        steps,
        "push",
        provider,
        store,
        &app_handle,
        &registry,
    )?;

    Ok(BranchPipelineResponse::running(session_id))
}

/// Push a branch to its remote.
///
/// Queues behind in-flight branch work instead of failing, so the response
/// reports whether the push started or is waiting on the branch queue.
///
/// `no_verify` ("Push without verifying") skips git's `pre-push` hook.
#[tauri::command(rename_all = "camelCase")]
pub async fn push_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    force: Option<bool>,
    no_verify: Option<bool>,
) -> Result<BranchPipelineResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_push_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
        PushVariant::from_request(force, no_verify),
    )
    .await
}

// =============================================================================
// Deferred push ("Push after current work")
// =============================================================================
//
// A push armed on the branch row (`branches.deferred_push`, see
// `store::DeferredPush`) that fires once the branch and its project are idle.
// It is a flag rather than a queued push row because a queued row sits in the
// branch FIFO — it would block work queued behind it, and the queue is blind to
// project sessions. The fired push is an ordinary push pipeline, so the
// per-variant dedupe, the prompts and the `push-completed` handling all apply.

impl PushVariant {
    /// The variant a deferred push was armed with.
    fn of_deferred(deferred: &store::DeferredPush) -> Self {
        Self {
            force: deferred.force,
            no_verify: deferred.no_verify,
        }
    }
}

/// What `defer_branch_push` did: armed the flag, or — the branch and project
/// were already idle when it ran — pushed right away, in which case `push` is
/// the push's start-or-queue response.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeferredPushResponse {
    pub armed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push: Option<BranchPipelineResponse>,
}

/// The sessions a deferred push is waiting on: the branch's sessions
/// `running`, `disconnected` or `queued` (commit, note, review, pipeline,
/// background hold or Blox turn alike), and the project's own sessions in any
/// of those states.
///
/// "Fully idle" rather than "the sessions running at arm time" on purpose: a
/// project session can enqueue branch work through the project MCP right
/// before it ends, and the branch check catches that. The ids are kept so the
/// drain can say what it is waiting on.
#[derive(Debug, Default, PartialEq, Eq)]
struct DeferredPushBlockers {
    branch_sessions: Vec<String>,
    project_sessions: Vec<String>,
}

impl DeferredPushBlockers {
    fn is_empty(&self) -> bool {
        self.branch_sessions.is_empty() && self.project_sessions.is_empty()
    }
}

fn deferred_push_blockers(
    store: &Arc<Store>,
    branch: &store::Branch,
) -> Result<DeferredPushBlockers, String> {
    let busy = store
        .get_busy_sessions_for_branch(&branch.id)
        .map_err(|e| e.to_string())?;
    let queued = store
        .get_queued_sessions_for_branch(&branch.id)
        .map_err(|e| e.to_string())?;
    Ok(DeferredPushBlockers {
        branch_sessions: busy
            .into_iter()
            .chain(queued)
            .map(|session| session.id)
            .collect(),
        project_sessions: store
            .project_session_ids_in_flight(&branch.project_id)
            .map_err(|e| e.to_string())?,
    })
}

/// Whether the branch and its project have nothing in flight (see
/// [`deferred_push_blockers`]).
fn branch_and_project_idle(store: &Arc<Store>, branch: &store::Branch) -> Result<bool, String> {
    Ok(deferred_push_blockers(store, branch)?.is_empty())
}

/// Whether a push of `variant` completed successfully on the branch since
/// `since` (ms), so a deferred push armed then has nothing left to do.
///
/// The case: a push queued behind the current work *and* "Push after current
/// work" armed on top of it. The queued push runs first and the deferred one
/// would otherwise fire a second, redundant push the moment the branch goes
/// idle. "Successfully" is the pipeline's own verdict — completed without the
/// agent, or every step succeeded or was skipped — since a rejection the agent
/// recovered from may have pushed something else.
fn push_of_variant_completed_since(
    store: &Arc<Store>,
    branch_id: &str,
    variant: PushVariant,
    since: i64,
) -> Result<Option<String>, String> {
    let completed = store
        .get_completed_sessions_for_branch_since(branch_id, since)
        .map_err(|e| e.to_string())?;
    Ok(completed
        .into_iter()
        .find(|session| {
            session.pipeline.as_ref().is_some_and(|pipeline| {
                pipeline.kind.as_ref() == Some(&PipelineKind::Push)
                    && PushVariant::of(pipeline) == variant
                    && (pipeline.completed_without_ai
                        || pipeline.steps.iter().all(|step| {
                            matches!(step.status, StepStatus::Succeeded | StepStatus::Skipped)
                        }))
            })
        })
        .map(|session| session.id))
}

/// Arm `variant` to push once the branch and its project are idle — or, when
/// they already are, push now through `push`.
///
/// The check-and-set runs under the branch launch lock, so a session start
/// racing it either lands before the check (the push is armed) or after (the
/// push starts, and its own re-check queues it behind the new session). Pushing
/// immediately closes the gap where work finishes between the menu rendering
/// and the click, which would otherwise leave an armed flag nothing kicks.
///
/// Arming again overwrites the variant. `push` is injected so tests can drive
/// the decision without a runner.
pub(crate) async fn defer_branch_push_with<F, Fut>(
    store: &Arc<Store>,
    branch_id: &str,
    variant: PushVariant,
    push: F,
) -> Result<DeferredPushResponse, String>
where
    F: FnOnce(PushVariant) -> Fut,
    Fut: Future<Output = Result<BranchPipelineResponse, String>>,
{
    let armed = {
        let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
        let _guard = launch_lock.lock().unwrap();
        let branch = store
            .get_branch(branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
        if branch_and_project_idle(store, &branch)? {
            false
        } else {
            store
                .set_branch_deferred_push(
                    branch_id,
                    Some(store::DeferredPush {
                        force: variant.force,
                        no_verify: variant.no_verify,
                        requested_at: store::now_timestamp(),
                    }),
                )
                .map_err(|e| e.to_string())?;
            true
        }
    };
    if armed {
        return Ok(DeferredPushResponse {
            armed: true,
            push: None,
        });
    }
    let response = push(variant).await?;
    Ok(DeferredPushResponse {
        armed: false,
        push: Some(response),
    })
}

/// [`defer_branch_push_with`] whose immediate push is the ordinary
/// start-or-queue push. `provider` only matters for that immediate push; a
/// push fired later by the drain runs with the backend default, like a queued
/// row's does.
pub(crate) async fn defer_branch_push_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    variant: PushVariant,
) -> Result<DeferredPushResponse, String> {
    let push_store = Arc::clone(&store);
    let push_branch_id = branch_id.clone();
    defer_branch_push_with(&store, &branch_id, variant, move |variant| {
        start_or_queue_push_pipeline_for_branch(
            push_store,
            registry,
            app_handle,
            push_branch_id,
            provider,
            variant,
        )
    })
    .await
}

/// Clear the branch's deferred push. A no-op when nothing is armed.
pub(crate) fn cancel_deferred_push_for_branch(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<(), String> {
    store
        .set_branch_deferred_push(branch_id, None)
        .map_err(|e| e.to_string())
}

/// What the drain tail finds when it looks at a branch's deferred push.
#[derive(Debug, PartialEq, Eq)]
enum DeferredPushCheck {
    /// Nothing armed, or no such branch.
    NotArmed,
    /// Armed, but the branch or project still has work in flight.
    Blocked(DeferredPushBlockers),
    /// Armed and idle, but a push of the same variant already completed
    /// since it was armed (the named session), so there is nothing to fire.
    Satisfied(store::DeferredPush, String),
    /// Armed, idle, and nothing has pushed since: fire it.
    Ready(store::DeferredPush),
}

fn check_deferred_push(store: &Arc<Store>, branch_id: &str) -> Result<DeferredPushCheck, String> {
    let Some(branch) = store.get_branch(branch_id).map_err(|e| e.to_string())? else {
        return Ok(DeferredPushCheck::NotArmed);
    };
    let Some(deferred) = branch.deferred_push else {
        return Ok(DeferredPushCheck::NotArmed);
    };
    let blockers = deferred_push_blockers(store, &branch)?;
    if !blockers.is_empty() {
        return Ok(DeferredPushCheck::Blocked(blockers));
    }
    if let Some(session_id) = push_of_variant_completed_since(
        store,
        branch_id,
        PushVariant::of_deferred(&deferred),
        deferred.requested_at,
    )? {
        return Ok(DeferredPushCheck::Satisfied(deferred, session_id));
    }
    Ok(DeferredPushCheck::Ready(deferred))
}

/// Fire the branch's deferred push through `fire` if one is armed, the
/// branch and project are idle, and no push of its variant has completed since
/// it was armed (that clears it instead). Returns whether a push was started;
/// while blocked, the sessions holding it are logged.
///
/// The flag is cleared *before* the push starts, so the push's own terminal
/// state (which kicks the branch again) cannot find it still armed and fire
/// twice; a failure to start re-arms it — unless the branch is gone or was
/// re-armed meanwhile — so the next kick retries. `fire` is injected so tests
/// can observe the variant without a runner.
pub(crate) async fn fire_deferred_push_with<F, Fut>(
    store: &Arc<Store>,
    branch_id: &str,
    fire: F,
) -> Result<bool, String>
where
    F: FnOnce(PushVariant) -> Fut,
    Fut: Future<Output = Result<BranchPipelineResponse, String>>,
{
    let deferred = match check_deferred_push(store, branch_id)? {
        DeferredPushCheck::NotArmed => return Ok(false),
        DeferredPushCheck::Blocked(blockers) => {
            log::info!(
                "[session_queue] deferred push on branch {branch_id} waits on branch sessions {:?} \
                 and project sessions {:?}",
                blockers.branch_sessions,
                blockers.project_sessions
            );
            return Ok(false);
        }
        DeferredPushCheck::Satisfied(deferred, session_id) => {
            store
                .set_branch_deferred_push(branch_id, None)
                .map_err(|e| e.to_string())?;
            log::info!(
                "[session_queue] deferred push on branch {branch_id} cleared: push session \
                 {session_id} (force: {}, no_verify: {}) completed since it was armed",
                deferred.force,
                deferred.no_verify
            );
            return Ok(false);
        }
        DeferredPushCheck::Ready(deferred) => deferred,
    };
    store
        .set_branch_deferred_push(branch_id, None)
        .map_err(|e| e.to_string())?;
    let variant = PushVariant::of_deferred(&deferred);
    match fire(variant).await {
        Ok(response) => {
            log::info!(
                "[session_queue] deferred push on branch {branch_id} fired as session {} ({:?}, force: {}, no_verify: {})",
                response.session_id,
                response.session_status,
                variant.force,
                variant.no_verify
            );
            Ok(true)
        }
        Err(e) => {
            let still_clear = store
                .get_branch(branch_id)
                .map_err(|e| e.to_string())?
                .is_some_and(|branch| branch.deferred_push.is_none());
            if still_clear {
                store
                    .set_branch_deferred_push(branch_id, Some(deferred))
                    .map_err(|e| e.to_string())?;
            }
            Err(e)
        }
    }
}

/// [`fire_deferred_push_with`] whose push is the ordinary start-or-queue
/// push. Called from the tail of `drain_branch_queue`, on the kicker's own
/// task, when nothing queued could start; `provider` is the backend default,
/// as for a queued row.
pub(crate) async fn fire_deferred_push_if_idle(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<bool, String> {
    let fire_store = Arc::clone(&store);
    let fire_branch_id = branch_id.clone();
    fire_deferred_push_with(&store, &branch_id, move |variant| {
        start_or_queue_push_pipeline_for_branch(
            fire_store,
            registry,
            app_handle,
            fire_branch_id,
            None,
            variant,
        )
    })
    .await
}

/// "Push after current work" and its variants: arm a push to run once the
/// branch and its project are idle, or push now if they already are.
#[tauri::command(rename_all = "camelCase")]
pub async fn defer_branch_push(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    force: Option<bool>,
    no_verify: Option<bool>,
) -> Result<DeferredPushResponse, String> {
    let store = get_store(&store)?;
    defer_branch_push_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
        PushVariant::from_request(force, no_verify),
    )
    .await
}

/// Clear a branch's deferred push.
#[tauri::command(rename_all = "camelCase")]
pub fn cancel_deferred_push(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    cancel_deferred_push_for_branch(&store, &branch_id)
}

/// Rebase a branch onto its base branch via a pipeline.
///
/// When `target` is `None` or `"base"`, rebases onto `origin/{base_branch}`
/// (the default behaviour used by the base-moved row and the `…` menu).
/// When `target` is `"origin"`, rebases onto `origin/{remote_branch}` so that
/// the local branch incorporates remote-only commits (used by the diverged row).
///
/// Queues behind in-flight branch work instead of failing, so the response
/// reports whether the rebase started or is waiting on the branch queue.
///
/// `then_force_push` ("Rebase and force push") also queues a force push behind
/// the rebase that only runs if the rebase finishes; its id comes back as
/// `follow_up_session_id`.
#[tauri::command(rename_all = "camelCase")]
pub async fn rebase_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    target: Option<String>,
    then_force_push: Option<bool>,
) -> Result<BranchPipelineResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_commit_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        PipelineKind::Rebase,
        provider,
        target,
        then_force_push.unwrap_or(false),
    )
    .await
}

/// Squash all commits on a branch into a single commit via a pipeline.
///
/// Fetches the latest base branch, captures the commit history, then uses
/// `git reset --soft $(git merge-base origin/{base} HEAD)` to collapse only the
/// branch's own commits into staged changes.
/// Hands off to AI to write a single conventional-commit message using the
/// original commit history as context.
///
/// Queues behind in-flight branch work instead of failing, so the response
/// reports whether the squash started or is waiting on the branch queue.
#[tauri::command(rename_all = "camelCase")]
pub async fn squash_commits(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<BranchPipelineResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_commit_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        PipelineKind::Squash,
        provider,
        None,
        false,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_at(steps: &[PipelineStep], index: usize) -> (&str, &str, &FailureStrategy) {
        match &steps[index] {
            PipelineStep::Command {
                label,
                command,
                on_failure,
            } => (label, command, on_failure),
            PipelineStep::AiHandoff { .. } => panic!("expected command step at index {index}"),
        }
    }

    fn ai_prompt_at(steps: &[PipelineStep], index: usize) -> (&str, &str) {
        match &steps[index] {
            PipelineStep::AiHandoff {
                label,
                prompt_template,
            } => (label, prompt_template),
            PipelineStep::Command { .. } => panic!("expected AI handoff step at index {index}"),
        }
    }

    fn test_registry() -> Arc<session_runner::SessionRegistry> {
        Arc::new(session_runner::SessionRegistry::new())
    }

    fn setup_branch_store() -> (Arc<Store>, store::Branch) {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        (store, branch)
    }

    /// Make the branch busy with a running note session on it, which is what
    /// `has_running_session_for_branch` looks for.
    fn start_running_note_session(store: &Arc<Store>, branch_id: &str) {
        let session = store::Session::new_running("write a note", Path::new("/tmp/staged-test"))
            .with_branch(branch_id);
        store.create_session(&session).unwrap();
        let note = store::Note::new(branch_id, "note", "").with_session(&session.id);
        store.create_note(&note).unwrap();
    }

    fn queue_pipeline(
        store: &Arc<Store>,
        branch_id: &str,
        kind: PipelineKind,
        target: Option<&str>,
    ) -> Option<String> {
        queue_commit_pipeline_if_branch_busy(store, branch_id, &kind, None, target, false)
            .unwrap()
            .map(|response| response.session_id)
    }

    /// The stale-lock reproduction. In web mode the request future owns the
    /// pull: hyper drops it the moment the client goes away (its HTTP/1
    /// half-close default closes the connection on a mid-request EOF), and a
    /// `running` Pull row with no runner behind it is what used to be left.
    /// That row is an exclusive kind, so every later note, commit, review and
    /// push on the branch queued behind it with nothing to ever release it.
    ///
    /// Poll the request once — far enough to insert the marker — drop it, let
    /// the pull finish, and the marker must still complete and the branch must
    /// still take a Note without queueing it.
    #[tokio::test]
    async fn dropping_the_request_future_mid_pull_no_longer_strands_the_marker() {
        let (store, branch) = setup_branch_store();
        let workdir = store::Workdir::new(&branch.project_id, "/tmp/staged-test-pull")
            .with_branch(&branch.id);
        store.create_workdir(&workdir).unwrap();

        let registry = test_registry();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let request = pull_or_queue_branch_with(
            Arc::clone(&store),
            Arc::clone(&registry),
            branch.id.clone(),
            move || {
                released.recv().map_err(|e| e.to_string())?;
                Ok(())
            },
            || {},
        );
        {
            let mut request = std::pin::pin!(request);
            std::future::poll_fn(|cx| {
                assert!(
                    std::future::Future::poll(request.as_mut(), cx).is_pending(),
                    "the pull must still be in flight after one poll"
                );
                std::task::Poll::Ready(())
            })
            .await;
            // The client is gone.
        }

        let marker = store
            .get_running_sessions()
            .unwrap()
            .into_iter()
            .find(|session| session.branch_id.as_deref() == Some(branch.id.as_str()))
            .expect("the marker row is inserted before the first await");
        // The marker never goes through the runner, so its liveness hold is
        // what keeps the stranded-session sweep off it for as long as the
        // pull takes — and the hold belongs to the pull task, not the
        // request future that was just dropped.
        assert!(
            registry.is_live(&marker.id),
            "the immediate pull's marker must be live in the registry while the pull runs"
        );
        release.send(()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let status = loop {
            let status = store.get_session(&marker.id).unwrap().unwrap().status;
            if status != store::SessionStatus::Running || std::time::Instant::now() > deadline {
                break status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(
            status,
            store::SessionStatus::Completed,
            "the marker must be released by the pull, not by the request future"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while registry.is_live(&marker.id) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            !registry.is_live(&marker.id),
            "the hold is released once the marker has its terminal state"
        );
        assert!(
            !crate::session_commands::should_queue_branch_session_start(
                &store,
                &branch.id,
                &crate::session_commands::BranchSessionType::Note,
            )
            .unwrap(),
            "a note on the branch must start, not queue behind a stale pull marker"
        );
    }

    #[test]
    fn idle_branch_runs_pipeline_immediately_instead_of_queueing() {
        let (store, branch) = setup_branch_store();

        assert_eq!(
            queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None),
            None
        );
        assert!(store
            .list_commits_for_branch(&branch.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn busy_branch_queues_pipeline_with_label_and_pending_commit() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let session_id = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None)
            .expect("rebase should queue behind the running note session");

        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.status, store::SessionStatus::Queued);
        // The timeline renders queued pipeline rows from this prompt, so it has
        // to read as the git action rather than a bare "Pending commit".
        assert_eq!(session.prompt, "Rebase branch");
        let pipeline = session.pipeline.unwrap();
        assert_eq!(pipeline.kind, Some(PipelineKind::Rebase));
        assert_eq!(pipeline.rebase_target, None);

        let pending: Vec<_> = store
            .list_commits_for_branch(&branch.id)
            .unwrap()
            .into_iter()
            .filter(|c| c.sha.is_none())
            .collect();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].session_id.as_deref(), Some(session_id.as_str()));
    }

    #[test]
    fn queued_squash_uses_squash_label() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let session_id = queue_pipeline(&store, &branch.id, PipelineKind::Squash, None).unwrap();

        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.prompt, "Squash commits");
        assert_eq!(session.pipeline.unwrap().kind, Some(PipelineKind::Squash));
    }

    #[test]
    fn repeated_click_reuses_the_already_queued_pipeline() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let first = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();
        let second = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn different_pipeline_kinds_queue_separately() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let rebase = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();
        let squash = queue_pipeline(&store, &branch.id, PipelineKind::Squash, None).unwrap();

        assert_ne!(rebase, squash);
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn rebase_onto_origin_is_not_deduped_against_rebase_onto_base() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let onto_base = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();
        let onto_origin =
            queue_pipeline(&store, &branch.id, PipelineKind::Rebase, Some("origin")).unwrap();

        assert_ne!(onto_base, onto_origin);
        let origin_session = store.get_session(&onto_origin).unwrap().unwrap();
        assert_eq!(
            origin_session.pipeline.unwrap().rebase_target.as_deref(),
            Some("feature")
        );

        // Clicking the diverged row's rebase again still dedupes.
        assert_eq!(
            queue_pipeline(&store, &branch.id, PipelineKind::Rebase, Some("origin")),
            Some(onto_origin)
        );
    }

    #[test]
    fn queued_pipeline_alone_keeps_the_branch_busy() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();

        // The note session finishes, but the still-queued rebase must keep a
        // newly requested squash on the queue rather than running it now.
        for session in store.get_running_sessions().unwrap() {
            store
                .update_session_status(&session.id, store::SessionStatus::Completed, None, None)
                .unwrap();
        }

        assert!(queue_pipeline(&store, &branch.id, PipelineKind::Squash, None).is_some());
    }

    const PUSH: PushVariant = PushVariant {
        force: false,
        no_verify: false,
    };
    const FORCE_PUSH: PushVariant = PushVariant {
        force: true,
        no_verify: false,
    };
    const NO_VERIFY_PUSH: PushVariant = PushVariant {
        force: false,
        no_verify: true,
    };
    const NO_VERIFY_FORCE_PUSH: PushVariant = PushVariant {
        force: true,
        no_verify: true,
    };

    fn queue_push(store: &Arc<Store>, branch_id: &str, variant: PushVariant) -> Option<String> {
        queue_push_pipeline_if_branch_busy(store, branch_id, None, variant).unwrap()
    }

    #[test]
    fn idle_branch_pushes_immediately_instead_of_queueing() {
        let (store, branch) = setup_branch_store();

        assert_eq!(queue_push(&store, &branch.id, PUSH), None);
        assert!(store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn busy_branch_queues_push_with_branch_link_and_no_artifact() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let session_id = queue_push(&store, &branch.id, PUSH)
            .expect("push should queue behind the running note session");

        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.status, store::SessionStatus::Queued);
        assert_eq!(session.prompt, PUSH_PROMPT);
        // The branch link is what keeps an artifact-less push on the queue.
        assert_eq!(session.branch_id.as_deref(), Some(branch.id.as_str()));
        let pipeline = session.pipeline.unwrap();
        assert_eq!(pipeline.kind, Some(PipelineKind::Push));
        assert!(!pipeline.push_force);

        // A pending commit would render as a failed commit once the push ends.
        assert!(store
            .list_commits_for_branch(&branch.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn repeated_push_click_reuses_the_already_queued_push() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let first = queue_push(&store, &branch.id, PUSH).unwrap();
        let second = queue_push(&store, &branch.id, PUSH).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn force_push_is_not_deduped_against_a_queued_normal_push() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let push = queue_push(&store, &branch.id, PUSH).unwrap();
        let force_push = queue_push(&store, &branch.id, FORCE_PUSH).unwrap();

        assert_ne!(push, force_push);
        let forced = store.get_session(&force_push).unwrap().unwrap();
        assert_eq!(forced.prompt, FORCE_PUSH_PROMPT);
        assert!(forced.pipeline.unwrap().push_force);
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            2
        );

        // Clicking force push again still dedupes.
        assert_eq!(queue_push(&store, &branch.id, FORCE_PUSH), Some(force_push));
    }

    #[test]
    fn no_verify_and_verifying_pushes_do_not_dedupe_into_each_other() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let variants = [PUSH, NO_VERIFY_PUSH, FORCE_PUSH, NO_VERIFY_FORCE_PUSH];
        let ids: Vec<String> = variants
            .iter()
            .map(|&variant| queue_push(&store, &branch.id, variant).unwrap())
            .collect();

        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), variants.len(), "{ids:?}");
        for (variant, id) in variants.iter().zip(&ids) {
            let session = store.get_session(id).unwrap().unwrap();
            assert_eq!(
                session.prompt,
                pipeline_prompt(&PipelineKind::Push, *variant)
            );
            assert_eq!(PushVariant::of(&session.pipeline.unwrap()), *variant);
            // A repeat request for the same variant still dedupes.
            assert_eq!(queue_push(&store, &branch.id, *variant).as_ref(), Some(id));
        }
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            variants.len()
        );
    }

    /// A no-verify push that waited behind other work must still skip the
    /// hook once the queue drains it, and the running row must still say so.
    #[test]
    fn queued_no_verify_push_is_rebuilt_with_the_flag_on_dequeue() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        for variant in [NO_VERIFY_PUSH, NO_VERIFY_FORCE_PUSH, PUSH] {
            let id = queue_push(&store, &branch.id, variant).unwrap();
            let queued = store.get_session(&id).unwrap().unwrap();

            let rebuilt = rebuild_queued_git_pipeline(
                &id,
                queued.pipeline.as_ref().unwrap(),
                "feature-renamed",
                "feature",
            )
            .unwrap();

            assert_eq!(rebuilt.session_type, "push");
            assert_eq!(rebuilt.prompt, queued.prompt);
            assert_eq!(PushVariant::of(&rebuilt.pipeline), variant);
            let (_, command, _) = command_at(&rebuilt.steps, 0);
            assert!(
                command.contains("origin feature-renamed:feature"),
                "{command}"
            );
            assert_eq!(
                command.contains("--no-verify"),
                variant.no_verify,
                "{command}"
            );
            assert_eq!(
                command.contains("--force-with-lease"),
                variant.force,
                "{command}"
            );
        }
    }

    #[test]
    fn rebuilt_pipeline_keeps_the_push_dependency_and_rejects_commit_kinds() {
        let queued = PipelineExecution::from_steps(&[])
            .with_kind(PipelineKind::Push)
            .with_push_force(true)
            .with_depends_on_session_id(Some("rebase-1".to_string()));
        let rebuilt = rebuild_queued_git_pipeline("push-1", &queued, "feature", "feature").unwrap();
        assert_eq!(
            rebuilt.pipeline.depends_on_session_id.as_deref(),
            Some("rebase-1")
        );
        assert_eq!(PushVariant::of(&rebuilt.pipeline), FORCE_PUSH);

        let pull = PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Pull);
        let rebuilt = rebuild_queued_git_pipeline("pull-1", &pull, "feature", "feature").unwrap();
        assert_eq!(rebuilt.session_type, "pull");
        assert_eq!(rebuilt.prompt, PULL_PROMPT);

        let rebase = PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase);
        assert!(rebuild_queued_git_pipeline("rebase-1", &rebase, "feature", "feature").is_err());
    }

    /// The prompt is how a push session is recognised once it ends:
    /// `session_completion` keys the `push-completed` event (and clearing the
    /// stale PR status) on it.
    #[test]
    fn every_push_prompt_is_recognised_as_a_push() {
        for variant in [PUSH, NO_VERIFY_PUSH, FORCE_PUSH, NO_VERIFY_FORCE_PUSH] {
            let prompt = pipeline_prompt(&PipelineKind::Push, variant);
            assert_eq!(
                crate::session_commands::infer_branch_resume_session_type(prompt),
                Some("push"),
                "{variant:?}: {prompt}"
            );
        }
        assert!(NO_VERIFY_PUSH_PROMPT.contains("--no-verify"));
        assert!(NO_VERIFY_PUSH_PROMPT.contains("Do not force push."));
        assert!(NO_VERIFY_FORCE_PUSH_PROMPT.contains("--no-verify"));
    }

    #[test]
    fn queued_push_alone_keeps_the_branch_busy() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        queue_push(&store, &branch.id, PUSH).unwrap();

        // The note session finishes, but the still-queued push must keep a newly
        // requested squash on the queue rather than running it now.
        for session in store.get_running_sessions().unwrap() {
            store
                .update_session_status(&session.id, store::SessionStatus::Completed, None, None)
                .unwrap();
        }

        assert!(queue_pipeline(&store, &branch.id, PipelineKind::Squash, None).is_some());
    }

    #[test]
    fn queued_rebase_keeps_a_later_push_on_the_queue() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();

        for session in store.get_running_sessions().unwrap() {
            store
                .update_session_status(&session.id, store::SessionStatus::Completed, None, None)
                .unwrap();
        }

        assert!(queue_push(&store, &branch.id, PUSH).is_some());
    }

    fn queue_rebase_then_force_push(store: &Arc<Store>, branch_id: &str) -> BranchPipelineResponse {
        queue_commit_pipeline_if_branch_busy(
            store,
            branch_id,
            &PipelineKind::Rebase,
            None,
            None,
            true,
        )
        .unwrap()
        .expect("a busy branch queues the rebase")
    }

    #[test]
    fn rebase_and_force_push_queues_a_push_gated_on_the_rebase() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let response = queue_rebase_then_force_push(&store, &branch.id);
        let push_id = response
            .follow_up_session_id
            .clone()
            .expect("the force push is queued alongside the rebase");

        let rebase = store.get_session(&response.session_id).unwrap().unwrap();
        let push = store.get_session(&push_id).unwrap().unwrap();
        assert_eq!(push.status, store::SessionStatus::Queued);
        assert_eq!(push.prompt, FORCE_PUSH_PROMPT);
        assert_eq!(push.branch_id.as_deref(), Some(branch.id.as_str()));
        let pipeline = push.pipeline.unwrap();
        assert_eq!(pipeline.kind, Some(PipelineKind::Push));
        assert!(pipeline.push_force);
        assert_eq!(
            pipeline.depends_on_session_id.as_deref(),
            Some(rebase.id.as_str())
        );
        // Strictly after the rebase, so a shared millisecond can't put the
        // push first in the queue or the timeline.
        assert!(push.created_at > rebase.created_at);
        let queued: Vec<_> = store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(queued, vec![rebase.id, push_id]);
    }

    #[test]
    fn repeated_rebase_and_force_push_reuses_both_queued_rows() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let first = queue_rebase_then_force_push(&store, &branch.id);
        let second = queue_rebase_then_force_push(&store, &branch.id);

        assert_eq!(first, second);
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn gated_and_unconditional_force_pushes_do_not_dedupe_into_each_other() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let gated = queue_rebase_then_force_push(&store, &branch.id)
            .follow_up_session_id
            .unwrap();
        let plain = queue_push(&store, &branch.id, FORCE_PUSH).unwrap();

        assert_ne!(gated, plain);
        let plain = store.get_session(&plain).unwrap().unwrap();
        assert_eq!(plain.pipeline.unwrap().depends_on_session_id, None);
        // And a plain rebase still dedupes onto the queued one, without a push.
        let rebase_again = queue_commit_pipeline_if_branch_busy(
            &store,
            &branch.id,
            &PipelineKind::Rebase,
            None,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(rebase_again.follow_up_session_id, None);
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            3
        );
    }

    /// On an idle branch the rebase runs now, but its push still queues: it is
    /// inserted in the same locked section as the running rebase row, before
    /// the runner could finish (and drain) it.
    #[test]
    fn running_rebase_still_gets_a_queued_push() {
        let (store, branch) = setup_branch_store();
        let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();
        let running = insert_running_commit_pipeline_session(
            &store,
            &test_registry(),
            &pipeline_context(&branch),
            PipelineKind::Rebase,
            &steps,
            None,
            None,
        )
        .unwrap();

        let response = with_follow_up_force_push_locked(
            &store,
            &branch.id,
            None,
            BranchPipelineResponse::running(running.session_id.clone()),
            true,
        )
        .unwrap();

        assert_eq!(response.session_status, BranchSessionLaunchStatus::Running);
        let push = store
            .get_session(&response.follow_up_session_id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(push.status, store::SessionStatus::Queued);
        assert_eq!(
            push.pipeline.unwrap().depends_on_session_id,
            Some(running.session_id)
        );
    }

    // -----------------------------------------------------------------------
    // Deferred push ("Push after current work")
    // -----------------------------------------------------------------------

    /// A running project-note session in `project_id`: the project-level
    /// work a deferred push waits for, invisible to the branch queries.
    fn start_running_project_note_session(store: &Arc<Store>, project_id: &str) -> String {
        let session = store::Session::new_running("plan", Path::new("/tmp/staged-test"));
        store.create_session(&session).unwrap();
        let note = store::ProjectNote::new(project_id, "Plan", "").with_session(&session.id);
        store.create_project_note(&note).unwrap();
        session.id
    }

    fn end_session(store: &Arc<Store>, session_id: &str) {
        store
            .update_session_status(session_id, store::SessionStatus::Completed, None, None)
            .unwrap();
    }

    fn deferred_push_of(store: &Arc<Store>, branch_id: &str) -> Option<store::DeferredPush> {
        store.get_branch(branch_id).unwrap().unwrap().deferred_push
    }

    /// Arm `variant` with an immediate push that must not run.
    async fn arm(store: &Arc<Store>, branch_id: &str, variant: PushVariant) {
        let response = defer_branch_push_with(store, branch_id, variant, |variant| {
            std::future::ready(Err(format!(
                "the branch was busy; nothing should push now ({variant:?})"
            )))
        })
        .await
        .unwrap();
        assert_eq!(
            response,
            DeferredPushResponse {
                armed: true,
                push: None
            }
        );
    }

    /// Run the drain tail's firing step with a push that only records the
    /// variant it was asked for.
    async fn fire(store: &Arc<Store>, branch_id: &str) -> (bool, Vec<PushVariant>) {
        let fired = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&fired);
        let started = fire_deferred_push_with(store, branch_id, move |variant| {
            recorder.lock().unwrap().push(variant);
            std::future::ready(Ok(BranchPipelineResponse::running(format!(
                "push-{}",
                recorder.lock().unwrap().len()
            ))))
        })
        .await
        .unwrap();
        let fired = fired.lock().unwrap().clone();
        (started, fired)
    }

    /// (a) A deferred push does not fire while a branch session is running:
    /// the flag stays armed, and the branch going idle is what fires it.
    #[tokio::test]
    async fn deferred_push_waits_for_the_branchs_running_session() {
        let (store, branch) = setup_branch_store();
        let note_session = {
            start_running_note_session(&store, &branch.id);
            store.get_running_sessions_for_branch(&branch.id).unwrap()[0]
                .id
                .clone()
        };

        arm(&store, &branch.id, PUSH).await;
        let armed = deferred_push_of(&store, &branch.id).expect("armed");
        assert_eq!(PushVariant::of_deferred(&armed), PUSH);
        assert!(armed.requested_at > 0);

        assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
        assert!(
            deferred_push_of(&store, &branch.id).is_some(),
            "still armed"
        );

        end_session(&store, &note_session);
        assert_eq!(fire(&store, &branch.id).await, (true, vec![PUSH]));
        assert_eq!(
            deferred_push_of(&store, &branch.id),
            None,
            "cleared once fired"
        );
        // The push's own terminal state kicks the branch again: nothing left.
        assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
    }

    /// (b) A running project-note session holds the push too — the branch
    /// queries cannot see it — and each of the four variants fires as armed,
    /// exactly once, when the project goes idle.
    #[tokio::test]
    async fn deferred_push_waits_for_the_projects_own_sessions_and_fires_each_variant_once() {
        for variant in [PUSH, FORCE_PUSH, NO_VERIFY_PUSH, NO_VERIFY_FORCE_PUSH] {
            let (store, branch) = setup_branch_store();
            let project_session = start_running_project_note_session(&store, &branch.project_id);
            assert!(!store.has_running_session_for_branch(&branch.id).unwrap());

            arm(&store, &branch.id, variant).await;
            assert_eq!(
                deferred_push_of(&store, &branch.id).map(|d| d.as_column_name()),
                Some(match (variant.force, variant.no_verify) {
                    (false, false) => "push",
                    (true, false) => "force",
                    (false, true) => "push-no-verify",
                    (true, true) => "force-no-verify",
                })
            );
            assert_eq!(
                fire(&store, &branch.id).await,
                (false, vec![]),
                "{variant:?}"
            );

            end_session(&store, &project_session);
            assert_eq!(
                fire(&store, &branch.id).await,
                (true, vec![variant]),
                "{variant:?}"
            );
            assert_eq!(deferred_push_of(&store, &branch.id), None);
            assert_eq!(
                fire(&store, &branch.id).await,
                (false, vec![]),
                "{variant:?}"
            );
        }
    }

    /// A project session that is only queued, or disconnected, holds the push
    /// like a running one.
    #[tokio::test]
    async fn deferred_push_waits_for_queued_and_disconnected_project_sessions() {
        for status in [
            store::SessionStatus::Queued,
            store::SessionStatus::Disconnected,
        ] {
            let (store, branch) = setup_branch_store();
            let project_session = start_running_project_note_session(&store, &branch.project_id);
            store
                .update_session_status(&project_session, status.clone(), None, None)
                .unwrap();

            arm(&store, &branch.id, PUSH).await;
            assert_eq!(
                fire(&store, &branch.id).await,
                (false, vec![]),
                "{status:?}"
            );
        }
    }

    /// (c) Arming on an idle branch pushes right away instead: the window
    /// between the menu rendering and the click would otherwise leave a flag
    /// nothing kicks.
    #[tokio::test]
    async fn deferring_on_an_idle_branch_pushes_immediately_without_arming() {
        let (store, branch) = setup_branch_store();
        let pushed = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&pushed);
        let response = defer_branch_push_with(&store, &branch.id, NO_VERIFY_FORCE_PUSH, {
            move |variant| {
                recorder.lock().unwrap().push(variant);
                std::future::ready(Ok(BranchPipelineResponse::running("push-now".to_string())))
            }
        })
        .await
        .unwrap();

        assert_eq!(
            response,
            DeferredPushResponse {
                armed: false,
                push: Some(BranchPipelineResponse::running("push-now".to_string())),
            }
        );
        assert_eq!(*pushed.lock().unwrap(), vec![NO_VERIFY_FORCE_PUSH]);
        assert_eq!(deferred_push_of(&store, &branch.id), None);
    }

    /// Run the queued push `session_id` to a deterministic success, as the
    /// pipeline runner records one: every step succeeded, no agent needed.
    fn complete_push_successfully(store: &Arc<Store>, session_id: &str) {
        let session = store.get_session(session_id).unwrap().unwrap();
        let mut pipeline = session.pipeline.expect("a push pipeline");
        for step in &mut pipeline.steps {
            step.status = StepStatus::Succeeded;
        }
        pipeline.completed_without_ai = true;
        store
            .update_session_pipeline(session_id, &pipeline)
            .unwrap();
        end_session(store, session_id);
    }

    /// Every push pipeline session on the branch, whatever its state.
    fn push_sessions_on(store: &Arc<Store>, branch_id: &str) -> Vec<String> {
        let mut sessions = store.get_busy_sessions_for_branch(branch_id).unwrap();
        sessions.extend(store.get_queued_sessions_for_branch(branch_id).unwrap());
        sessions.extend(
            store
                .get_completed_sessions_for_branch_since(branch_id, 0)
                .unwrap(),
        );
        sessions
            .into_iter()
            .filter(|session| {
                session
                    .pipeline
                    .as_ref()
                    .is_some_and(|pipeline| pipeline.kind.as_ref() == Some(&PipelineKind::Push))
            })
            .map(|session| session.id)
            .collect()
    }

    /// (d) A queued push of the same variant is work in flight, so the
    /// deferred push waits behind it rather than stacking a second push; the
    /// queue's own dedupe still folds repeat requests onto the queued row; and
    /// once that push has succeeded the deferred one has nothing left to do:
    /// it is cleared instead of fired, so one intent ends as one push session.
    #[tokio::test]
    async fn deferred_push_waits_behind_a_queued_push_instead_of_stacking() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        let queued = queue_push(&store, &branch.id, PUSH).expect("queued behind the note");

        arm(&store, &branch.id, PUSH).await;
        assert_eq!(queue_push(&store, &branch.id, PUSH), Some(queued.clone()));

        // The note ends; the queued push is still ahead.
        for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
            end_session(&store, &session.id);
        }
        assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
        assert!(deferred_push_of(&store, &branch.id).is_some());
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            1,
            "no second push row"
        );

        // The queued push runs and succeeds: the branch is idle, and the push
        // the user wanted has happened.
        complete_push_successfully(&store, &queued);
        assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
        assert_eq!(
            deferred_push_of(&store, &branch.id),
            None,
            "cleared by the completed push"
        );
        assert_eq!(
            push_sessions_on(&store, &branch.id),
            vec![queued],
            "exactly one push session"
        );
    }

    /// A completed push of a *different* variant does not stand in for the
    /// deferred one: the user asked for something the queued push did not do.
    #[tokio::test]
    async fn deferred_push_is_not_satisfied_by_a_completed_push_of_another_variant() {
        for (queued_variant, deferred_variant) in [
            (FORCE_PUSH, PUSH),
            (PUSH, NO_VERIFY_PUSH),
            (NO_VERIFY_PUSH, NO_VERIFY_FORCE_PUSH),
        ] {
            let (store, branch) = setup_branch_store();
            start_running_note_session(&store, &branch.id);
            let queued = queue_push(&store, &branch.id, queued_variant).expect("queued");
            arm(&store, &branch.id, deferred_variant).await;

            for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
                end_session(&store, &session.id);
            }
            complete_push_successfully(&store, &queued);

            assert_eq!(
                fire(&store, &branch.id).await,
                (true, vec![deferred_variant]),
                "{queued_variant:?} completed, {deferred_variant:?} deferred"
            );
            assert_eq!(deferred_push_of(&store, &branch.id), None);
        }
    }

    /// A same-variant push that did not succeed — cancelled while queued, or
    /// completed before the deferred push was armed — leaves the deferred push
    /// to fire.
    #[tokio::test]
    async fn deferred_push_fires_when_the_same_variant_push_was_cancelled_or_predates_it() {
        // Cancelled while queued.
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        let queued = queue_push(&store, &branch.id, PUSH).expect("queued");
        arm(&store, &branch.id, PUSH).await;
        for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
            end_session(&store, &session.id);
        }
        store
            .update_session_status(&queued, store::SessionStatus::Cancelled, None, None)
            .unwrap();
        assert_eq!(fire(&store, &branch.id).await, (true, vec![PUSH]));

        // Completed before the arm: the push the user asked for comes after it.
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        let earlier = queue_push(&store, &branch.id, PUSH).expect("queued");
        complete_push_successfully(&store, &earlier);
        // Timestamps are in ms: let one pass so the arm is strictly later.
        std::thread::sleep(std::time::Duration::from_millis(2));
        arm(&store, &branch.id, PUSH).await;
        for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
            end_session(&store, &session.id);
        }
        assert_eq!(fire(&store, &branch.id).await, (true, vec![PUSH]));
    }

    /// A push that fails to start leaves the flag armed for the next kick —
    /// unless the user re-armed something else meanwhile, or the branch is gone.
    #[tokio::test]
    async fn deferred_push_that_fails_to_start_stays_armed() {
        let (store, branch) = setup_branch_store();
        let project_session = start_running_project_note_session(&store, &branch.project_id);
        arm(&store, &branch.id, FORCE_PUSH).await;
        end_session(&store, &project_session);

        let err = fire_deferred_push_with(&store, &branch.id, |_| {
            std::future::ready(Err::<BranchPipelineResponse, _>("no remote".to_string()))
        })
        .await
        .unwrap_err();
        assert_eq!(err, "no remote");
        assert_eq!(
            deferred_push_of(&store, &branch.id).map(|d| PushVariant::of_deferred(&d)),
            Some(FORCE_PUSH)
        );

        // Re-armed with another variant while the start was failing: kept.
        let err = fire_deferred_push_with(&store, &branch.id, |_| async {
            store
                .set_branch_deferred_push(
                    &branch.id,
                    Some(store::DeferredPush {
                        force: false,
                        no_verify: true,
                        requested_at: 1,
                    }),
                )
                .unwrap();
            Err::<BranchPipelineResponse, _>("no remote".to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(err, "no remote");
        assert_eq!(
            deferred_push_of(&store, &branch.id).map(|d| PushVariant::of_deferred(&d)),
            Some(NO_VERIFY_PUSH)
        );
    }

    /// Arming again replaces the variant; cancelling clears it; a branch with
    /// nothing armed cancels to nothing.
    #[tokio::test]
    async fn deferred_push_is_replaced_by_rearming_and_cleared_by_cancel() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        arm(&store, &branch.id, PUSH).await;
        arm(&store, &branch.id, NO_VERIFY_FORCE_PUSH).await;
        assert_eq!(
            deferred_push_of(&store, &branch.id).map(|d| PushVariant::of_deferred(&d)),
            Some(NO_VERIFY_FORCE_PUSH)
        );

        cancel_deferred_push_for_branch(&store, &branch.id).unwrap();
        assert_eq!(deferred_push_of(&store, &branch.id), None);
        cancel_deferred_push_for_branch(&store, &branch.id).unwrap();

        // Cancelled: a kick finds nothing to fire even once idle.
        for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
            end_session(&store, &session.id);
        }
        assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
    }

    /// The flag round-trips through the store, on create and on update, and
    /// lists per project.
    #[test]
    fn deferred_push_round_trips_through_the_branch_row() {
        let (store, branch) = setup_branch_store();
        let other_project = store::Project::new("other/repo");
        store.create_project(&other_project).unwrap();
        let mut created_armed = store::Branch::new(&other_project.id, "armed-on-create", "main");
        created_armed.deferred_push = Some(store::DeferredPush {
            force: true,
            no_verify: true,
            requested_at: 1234,
        });
        store.create_branch(&created_armed).unwrap();

        assert_eq!(
            store
                .get_branch(&created_armed.id)
                .unwrap()
                .unwrap()
                .deferred_push,
            created_armed.deferred_push
        );
        assert_eq!(deferred_push_of(&store, &branch.id), None);

        let armed = store::DeferredPush {
            force: false,
            no_verify: true,
            requested_at: 99,
        };
        store
            .set_branch_deferred_push(&branch.id, Some(armed))
            .unwrap();
        assert_eq!(deferred_push_of(&store, &branch.id), Some(armed));
        assert_eq!(store.branch_ids_with_deferred_push(None).unwrap().len(), 2);
        assert_eq!(
            store
                .branch_ids_with_deferred_push(Some(&branch.project_id))
                .unwrap(),
            vec![branch.id.clone()]
        );

        store.set_branch_deferred_push(&branch.id, None).unwrap();
        assert_eq!(deferred_push_of(&store, &branch.id), None);
        assert_eq!(
            store
                .branch_ids_with_deferred_push(Some(&branch.project_id))
                .unwrap(),
            Vec::<String>::new()
        );

        for name in ["push", "force", "push-no-verify", "force-no-verify"] {
            let parsed = store::DeferredPush::parse(name, 7).unwrap();
            assert_eq!(parsed.as_column_name(), name);
            assert_eq!(parsed.requested_at, 7);
        }
        assert_eq!(store::DeferredPush::parse("later", 7), None);
    }

    /// A mock app whose web broadcast channel is managed, so a test can read
    /// back exactly what `emit_to_all` sent to browser clients.
    fn mock_app_with_web_events() -> (
        tauri::App<tauri::test::MockRuntime>,
        tokio::sync::broadcast::Receiver<crate::web_server::WebEvent>,
    ) {
        use tauri::Manager;
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        let (tx, rx) = tokio::sync::broadcast::channel::<crate::web_server::WebEvent>(8);
        app.manage(tx);
        (app, rx)
    }

    /// The running rebase row goes in before its push, so a push insert that
    /// fails must not leave a `running` rebase nobody is running — and every
    /// client must hear about the failure, since the requesting one only
    /// toasts the error and nobody else would otherwise refresh the timeline.
    #[test]
    fn running_rebase_is_failed_when_its_push_cannot_be_queued() {
        let (store, branch) = setup_branch_store();
        let (app, mut web_events) = mock_app_with_web_events();
        let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();
        let running = insert_running_commit_pipeline_session(
            &store,
            &test_registry(),
            &pipeline_context(&branch),
            PipelineKind::Rebase,
            &steps,
            None,
            None,
        )
        .unwrap();

        // The push insert looks the branch up by id, so an unknown id is a
        // store failure after the rebase row already exists.
        let err = with_follow_up_force_push_or_fail_rebase_locked(
            &store,
            app.handle(),
            "missing-branch",
            &branch.project_id,
            None,
            &running.session_id,
            true,
        )
        .unwrap_err();
        assert!(err.contains("Branch not found"), "{err}");

        let rebase = store.get_session(&running.session_id).unwrap().unwrap();
        assert_eq!(rebase.status, store::SessionStatus::Error);
        assert_eq!(rebase.error_message.as_deref(), Some(err.as_str()));
        assert!(store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .is_empty());

        // The same terminal event the dead-session sweep would have emitted,
        // so timelines invalidate now rather than on the next refresh.
        let event = web_events.try_recv().expect("the failure is announced");
        assert_eq!(event.event_name, "session-status-changed");
        let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
        assert_eq!(payload["payload"]["sessionId"], running.session_id);
        assert_eq!(payload["payload"]["status"], "error");
        assert_eq!(payload["payload"]["errorMessage"], err);
        assert_eq!(payload["payload"]["branchId"], "missing-branch");
        assert_eq!(payload["payload"]["projectId"], branch.project_id);

        // Without a push to queue there is nothing that can fail, and the
        // rebase is left running for the runner, with nothing announced.
        let running = insert_running_commit_pipeline_session(
            &store,
            &test_registry(),
            &pipeline_context(&branch),
            PipelineKind::Rebase,
            &steps,
            None,
            None,
        )
        .unwrap();
        with_follow_up_force_push_or_fail_rebase_locked(
            &store,
            app.handle(),
            "missing-branch",
            &branch.project_id,
            None,
            &running.session_id,
            false,
        )
        .unwrap();
        assert_eq!(
            store
                .get_session(&running.session_id)
                .unwrap()
                .unwrap()
                .status,
            store::SessionStatus::Running
        );
        assert!(web_events.try_recv().is_err());
    }

    #[test]
    fn follow_up_session_id_is_omitted_from_plain_responses() {
        let json = serde_json::to_string(&BranchPipelineResponse::queued("s".into())).unwrap();
        assert!(!json.contains("followUpSessionId"), "{json}");
    }

    fn pipeline_context(branch: &store::Branch) -> BranchPipelineContext {
        BranchPipelineContext {
            branch: branch.clone(),
            working_dir: PathBuf::from("/tmp/staged-test"),
            workspace_name: None,
            remote_working_dir: None,
        }
    }

    #[test]
    fn inserting_a_running_pipeline_is_what_makes_the_branch_busy() {
        let (store, branch) = setup_branch_store();
        let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();

        // The pre-flight check sees an idle branch and lets the rebase run now.
        assert_eq!(
            queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None),
            None
        );
        insert_running_commit_pipeline_session(
            &store,
            &test_registry(),
            &pipeline_context(&branch),
            PipelineKind::Rebase,
            &steps,
            None,
            None,
        )
        .unwrap();

        // The rows that insert wrote are what the re-check at insert time reads, so
        // an action that raced it queues instead of also starting.
        assert!(queue_commit_pipeline_locked(
            &store,
            &branch.id,
            &PipelineKind::Squash,
            None,
            None
        )
        .unwrap()
        .is_some());
        assert!(queue_push(&store, &branch.id, PUSH).is_some());
    }

    #[test]
    fn inserting_a_running_push_is_what_makes_the_branch_busy() {
        let (store, branch) = setup_branch_store();
        let steps = build_push_pipeline_steps(&branch.branch_name, &branch.branch_name, PUSH);

        insert_running_push_pipeline_session(
            &store,
            &test_registry(),
            &pipeline_context(&branch),
            PUSH,
            &steps,
            None,
        )
        .unwrap();

        // A push has no artifact, so `branch_id` is what the re-check keys on.
        assert!(
            queue_push_pipeline_locked(&store, &branch.id, None, FORCE_PUSH)
                .unwrap()
                .is_some()
        );
        assert!(queue_commit_pipeline_locked(
            &store,
            &branch.id,
            &PipelineKind::Rebase,
            None,
            None
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn push_steps_are_rebuilt_from_the_current_branch_name_and_force_flag() {
        // Same local and remote name: the refspec is still explicit.
        let normal = build_push_pipeline_steps("feature", "feature", PUSH);
        let (label, command, on_failure) = command_at(&normal, 0);
        assert_eq!(label, "Push to remote");
        assert!(command.contains("-u origin feature:feature"));
        assert!(!command.contains("--force-with-lease"));
        assert!(matches!(
            on_failure,
            FailureStrategy::Abort { marker } if marker.as_deref() == Some("non-fast-forward")
        ));

        // The drain path re-derives the refspec, so a branch renamed while the
        // push waited pushes the new local name rather than the queued one.
        let forced = build_push_pipeline_steps("feature-renamed", "feature", FORCE_PUSH);
        let (_, forced_command, forced_on_failure) = command_at(&forced, 0);
        assert!(forced_command.contains("-u origin feature-renamed:feature --force-with-lease"));
        // The handoff spells the refspec out so an agent doesn't retry with
        // `git push origin feature-renamed` and create a stray remote branch.
        let FailureStrategy::HandoffToAi { prompt_template } = forced_on_failure else {
            panic!("expected force push to hand off to an agent");
        };
        assert!(
            prompt_template
                .contains("`git push -u origin feature-renamed:feature --force-with-lease`"),
            "{prompt_template}"
        );
        assert!(
            prompt_template.contains("{step_outputs}"),
            "{prompt_template}"
        );
    }

    /// The bug this guards against: a PR import whose head branch was already
    /// checked out elsewhere gets a de-duplicated local name (`feature-x-2`).
    /// Pushing must still land on the PR's branch, not create
    /// `origin/feature-x-2`.
    #[test]
    fn no_verify_push_steps_skip_the_hook_in_both_fallback_halves() {
        let plain = build_push_pipeline_steps("feature", "feature", PUSH);
        let (_, command, _) = command_at(&plain, 0);
        assert!(!command.contains("--no-verify"), "{command}");

        let no_verify = build_push_pipeline_steps("feature", "feature", NO_VERIFY_PUSH);
        let (_, command, on_failure) = command_at(&no_verify, 0);
        // Once for the SSH attempt, once for the HTTPS retry.
        assert_eq!(
            command
                .matches("push --no-verify -u origin feature:feature")
                .count(),
            2,
            "{command}"
        );
        assert!(!command.contains("--force-with-lease"), "{command}");
        // Still aborts on a rejection, so the force-push dialog can offer the
        // no-verify force push.
        assert!(matches!(
            on_failure,
            FailureStrategy::Abort { marker } if marker.as_deref() == Some("non-fast-forward")
        ));

        let forced = build_push_pipeline_steps("feature-renamed", "feature", NO_VERIFY_FORCE_PUSH);
        let (_, command, on_failure) = command_at(&forced, 0);
        assert_eq!(
            command
                .matches("push --no-verify -u origin feature-renamed:feature --force-with-lease")
                .count(),
            2,
            "{command}"
        );
        // The agent's retry must skip the hook too.
        let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
            panic!("expected force push to hand off to an agent");
        };
        assert!(
            prompt_template.contains(
                "`git push --no-verify -u origin feature-renamed:feature --force-with-lease`"
            ),
            "{prompt_template}"
        );
    }

    #[test]
    fn deduplicated_pr_branch_pushes_to_the_pr_head_not_its_local_name() {
        let steps = build_push_pipeline_steps("feature-x-2", "feature-x", PUSH);
        let (_, command, _) = command_at(&steps, 0);
        assert!(command.contains("git push -u origin feature-x-2:feature-x"));
        assert!(!command.contains("origin feature-x-2 "));
        assert!(!command.contains("feature-x-2:feature-x-2"));
    }

    #[test]
    fn rebase_onto_origin_targets_the_branch_the_checkout_pushes_to() {
        let (store, branch) = setup_branch_store();
        // No worktree yet: the branch resolves to its own name.
        let remote_branch = crate::branches::resolve_remote_branch_sync(&store, &branch);
        assert_eq!(remote_branch, branch.branch_name);
        assert_eq!(
            rebase_ref_for_target(&branch, &remote_branch, Some("origin")),
            branch.branch_name
        );

        // A de-duplicated PR import whose checkout tracks the PR head:
        // "rebase onto origin" targets the PR head, not origin/feature-x-2.
        let deduplicated = store::Branch::new(&branch.project_id, "feature-x-2", "main").with_pr(7);
        assert_eq!(
            rebase_ref_for_target(&deduplicated, "feature-x", Some("origin")),
            "feature-x"
        );
        assert_eq!(
            rebase_ref_for_target(&deduplicated, "feature-x", None),
            "main"
        );

        let steps =
            build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "feature-x").unwrap();
        let (_, rebase_command, _) = command_at(&steps, 1);
        assert!(rebase_command.contains("origin/feature-x"));
        assert!(!rebase_command.contains("feature-x-2"));
    }

    /// A queued "rebase onto origin" marks its row with the branch's local
    /// name even when the checkout tracks a differently named PR head: the
    /// branch on origin is resolved when the row is dequeued, never replayed
    /// from the marker.
    #[test]
    fn queued_rebase_onto_origin_marks_the_local_name_not_the_resolved_remote() {
        use crate::test_utils::{run_git_in, TempDir, TempGitRepo};

        let seed = TempGitRepo::new();
        seed.write_file("file.txt", "base\n");
        seed.commit("base");
        seed.run_git(&["branch", "feature-x"]);
        let origin = TempDir::new("origin");
        let origin_str = origin.path().to_str().unwrap();
        run_git_in(seed.path(), &["clone", "--bare", ".", origin_str]);
        let clone = TempDir::new("clone");
        run_git_in(
            origin.path(),
            &["clone", origin_str, clone.path().to_str().unwrap()],
        );
        run_git_in(
            clone.path(),
            &[
                "checkout",
                "--no-track",
                "-b",
                "feature-x-2",
                "origin/feature-x",
            ],
        );
        run_git_in(
            clone.path(),
            &["branch", "-u", "origin/feature-x", "feature-x-2"],
        );

        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("acme/widgets");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new(&project.id, "feature-x-2", "main").with_pr(7);
        store.create_branch(&branch).unwrap();
        store
            .create_workdir(
                &store::Workdir::new(&project.id, clone.path().to_str().unwrap())
                    .with_branch(&branch.id),
            )
            .unwrap();
        assert_eq!(
            crate::branches::resolve_remote_branch_sync(&store, &branch),
            "feature-x"
        );

        start_running_note_session(&store, &branch.id);
        let queued =
            queue_pipeline(&store, &branch.id, PipelineKind::Rebase, Some("origin")).unwrap();
        let session = store.get_session(&queued).unwrap().unwrap();
        assert_eq!(
            session.pipeline.unwrap().rebase_target.as_deref(),
            Some("feature-x-2")
        );
        // The marker still tells "onto origin" apart from "onto base".
        assert_eq!(
            queue_pipeline(&store, &branch.id, PipelineKind::Rebase, Some("origin")),
            Some(queued.clone())
        );
        assert_ne!(
            queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None),
            Some(queued)
        );
    }

    #[test]
    fn commit_pipeline_steps_reject_the_git_pipeline_kinds() {
        let push_err =
            build_commit_pipeline_steps(&PipelineKind::Push, "main", "main").unwrap_err();
        let pull_err =
            build_commit_pipeline_steps(&PipelineKind::Pull, "main", "main").unwrap_err();

        assert_eq!(push_err, "Push is not a commit pipeline");
        assert_eq!(pull_err, "Pull is not a commit pipeline");
    }

    /// The queued session id, or `None` when the branch was idle and the pull
    /// claimed it to run now.
    fn queue_pull(store: &Arc<Store>, branch_id: &str) -> Option<String> {
        match claim_or_queue_pull_for_branch(store, &test_registry(), branch_id).unwrap() {
            PullDisposition::Queued(session_id) => Some(session_id),
            PullDisposition::RunningNow { .. } => None,
        }
    }

    fn claim_pull(store: &Arc<Store>, branch_id: &str) -> String {
        match claim_or_queue_pull_for_branch(store, &test_registry(), branch_id).unwrap() {
            PullDisposition::RunningNow { session_id, .. } => session_id,
            PullDisposition::Queued(session_id) => {
                panic!("expected an immediate pull, got queued session {session_id}")
            }
        }
    }

    #[test]
    fn idle_branch_pulls_immediately_instead_of_queueing() {
        let (store, branch) = setup_branch_store();

        assert_eq!(queue_pull(&store, &branch.id), None);
        assert!(store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn immediate_pull_claims_the_branch_with_a_running_marker_session() {
        let (store, branch) = setup_branch_store();

        let session_id = claim_pull(&store, &branch.id);

        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.status, store::SessionStatus::Running);
        // The branch link and pipeline kind are what make the pull visible to the
        // queue; it creates no artifact, exactly like a push.
        assert_eq!(session.branch_id.as_deref(), Some(branch.id.as_str()));
        assert_eq!(
            session.pipeline.unwrap().kind,
            Some(store::PipelineKind::Pull)
        );
        assert!(store
            .list_commits_for_branch(&branch.id)
            .unwrap()
            .is_empty());
        assert!(store.has_running_session_for_branch(&branch.id).unwrap());
    }

    #[test]
    fn a_pull_in_flight_keeps_other_branch_work_off_the_worktree() {
        let (store, branch) = setup_branch_store();
        claim_pull(&store, &branch.id);

        // A commit pipeline or a second git action requested mid-pull has to wait
        // rather than race the fast-forward in the same worktree.
        assert!(queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).is_some());
        assert!(queue_push(&store, &branch.id, PUSH).is_some());
        assert!(queue_pull(&store, &branch.id).is_some());
    }

    #[test]
    fn finishing_an_immediate_pull_frees_the_branch_and_records_the_failure() {
        let (store, branch) = setup_branch_store();

        let ok = claim_pull(&store, &branch.id);
        finish_immediate_pull_session(&store, &ok, None);
        let completed = store.get_session(&ok).unwrap().unwrap();
        assert_eq!(completed.status, store::SessionStatus::Completed);
        assert!(!store.has_running_session_for_branch(&branch.id).unwrap());

        let failed_id = claim_pull(&store, &branch.id);
        finish_immediate_pull_session(
            &store,
            &failed_id,
            Some("Cannot pull with uncommitted changes"),
        );
        let failed = store.get_session(&failed_id).unwrap().unwrap();
        assert_eq!(failed.status, store::SessionStatus::Error);
        assert_eq!(
            failed.error_message.as_deref(),
            Some("Cannot pull with uncommitted changes")
        );
        assert!(!store.has_running_session_for_branch(&branch.id).unwrap());
    }

    #[test]
    fn busy_branch_queues_pull_with_branch_link_and_no_artifact() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let session_id =
            queue_pull(&store, &branch.id).expect("pull should queue behind the running note");

        let session = store.get_session(&session_id).unwrap().unwrap();
        assert_eq!(session.status, store::SessionStatus::Queued);
        assert_eq!(session.prompt, PULL_PROMPT);
        // Like a queued push, the branch link is what keeps an artifact-less
        // pull on the queue.
        assert_eq!(session.branch_id.as_deref(), Some(branch.id.as_str()));
        assert_eq!(
            session.pipeline.unwrap().kind,
            Some(store::PipelineKind::Pull)
        );
        assert!(store
            .list_commits_for_branch(&branch.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn repeated_pull_click_reuses_the_already_queued_pull() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let first = queue_pull(&store, &branch.id).unwrap();
        let second = queue_pull(&store, &branch.id).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn pull_and_push_queue_separately() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let pull = queue_pull(&store, &branch.id).unwrap();
        let push = queue_push(&store, &branch.id, PUSH).unwrap();

        assert_ne!(pull, push);
        assert_eq!(
            store
                .get_queued_sessions_for_branch(&branch.id)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn queued_pull_alone_keeps_the_branch_busy() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        queue_pull(&store, &branch.id).unwrap();

        // The note session finishes, but the still-queued pull must keep a newly
        // requested push on the queue rather than running it now.
        for session in store.get_running_sessions().unwrap() {
            store
                .update_session_status(&session.id, store::SessionStatus::Completed, None, None)
                .unwrap();
        }

        assert!(queue_push(&store, &branch.id, PUSH).is_some());
    }

    #[test]
    fn pull_steps_fast_forward_to_the_pinned_remote_branch() {
        // A de-duplicated PR import pulls from the PR head, never from the
        // (non-existent) origin/<local name>.
        let steps = build_pull_pipeline_steps("feature-x");
        let (_, fetch_command, _) = command_at(&steps, 0);
        assert!(fetch_command.contains("git fetch origin feature-x"));
        let (_, merge_command, _) = command_at(&steps, 1);
        assert_eq!(merge_command, "git merge --ff-only origin/feature-x");
    }

    #[test]
    fn pull_steps_fetch_then_fast_forward_and_never_hand_off() {
        // The drain path re-derives the ref, so a branch renamed while the pull
        // waited fast-forwards the new name rather than the queued one.
        let steps = build_pull_pipeline_steps("feature-renamed");

        let (fetch_label, fetch_command, fetch_on_failure) = command_at(&steps, 0);
        assert_eq!(fetch_label, "Fetch origin/feature-renamed");
        assert!(fetch_command.contains("git fetch origin feature-renamed"));
        assert!(matches!(
            fetch_on_failure,
            FailureStrategy::Abort { marker: None }
        ));

        let (merge_label, merge_command, merge_on_failure) = command_at(&steps, 1);
        assert_eq!(merge_label, "Fast-forward to origin/feature-renamed");
        assert_eq!(merge_command, "git merge --ff-only origin/feature-renamed");
        // A diverged branch is the user's call to make, not an agent's.
        assert!(matches!(
            merge_on_failure,
            FailureStrategy::Abort { marker: None }
        ));
        assert_eq!(steps.len(), 2);
    }

    fn branch_task(
        outcome: BranchRefresh,
    ) -> tokio::task::JoinHandle<Result<BranchRefresh, String>> {
        tokio::spawn(async move { Ok(outcome) })
    }

    #[tokio::test]
    async fn collect_branch_refresh_results_tolerates_partial_task_failure() {
        let tasks = vec![
            branch_task(BranchRefresh::Refreshed),
            tokio::spawn(async {
                panic!("simulated branch task panic");
                #[allow(unreachable_code)]
                Ok::<BranchRefresh, String>(BranchRefresh::Refreshed)
            }),
            branch_task(BranchRefresh::Failed),
        ];

        let refreshed_count = collect_branch_refresh_results(tasks).await.unwrap();

        assert_eq!(refreshed_count, 1);
    }

    #[tokio::test]
    async fn collect_branch_refresh_results_fails_when_all_tasks_fail() {
        let tasks = vec![
            tokio::spawn(async {
                Err::<BranchRefresh, String>("simulated semaphore failure".to_string())
            }),
            tokio::spawn(async {
                panic!("simulated branch task panic");
                #[allow(unreachable_code)]
                Ok::<BranchRefresh, String>(BranchRefresh::Refreshed)
            }),
        ];

        let err = collect_branch_refresh_results(tasks).await.unwrap_err();

        assert!(err.contains("all PR status refresh tasks failed"));
        assert!(err.contains("simulated semaphore failure"));
    }

    #[tokio::test]
    async fn collect_branch_refresh_results_fails_when_all_branches_fail() {
        let tasks = vec![
            branch_task(BranchRefresh::Failed),
            branch_task(BranchRefresh::Failed),
        ];

        let err = collect_branch_refresh_results(tasks).await.unwrap_err();

        assert!(err.contains("all PR status refresh tasks failed"));
        assert!(err.contains("2 branch refreshes failed"));
    }

    #[tokio::test]
    async fn collect_branch_refresh_results_does_not_fail_a_skipped_write() {
        // Every PR branch relinked or unlinked mid-poll: nothing refreshed,
        // but nothing failed either, so the poll scheduler records a success.
        let tasks = vec![
            branch_task(BranchRefresh::Skipped),
            branch_task(BranchRefresh::Skipped),
        ];
        assert_eq!(collect_branch_refresh_results(tasks).await.unwrap(), 0);

        // Alongside a refresh it isn't counted as one.
        let tasks = vec![
            branch_task(BranchRefresh::Refreshed),
            branch_task(BranchRefresh::Skipped),
        ];
        assert_eq!(collect_branch_refresh_results(tasks).await.unwrap(), 1);

        // Alongside a failure, only the failure is reported.
        let tasks = vec![
            branch_task(BranchRefresh::Skipped),
            branch_task(BranchRefresh::Failed),
        ];
        let err = collect_branch_refresh_results(tasks).await.unwrap_err();
        assert!(err.contains("1 branch refreshes failed"));
    }

    #[test]
    fn create_pr_pipeline_fetches_base_and_uses_origin_merge_base_for_context() {
        let steps = build_create_pr_pipeline_steps(
            "pull request",
            "main",
            "",
            "feature-branch-2",
            "feature-branch",
        );

        assert_eq!(steps.len(), 5);

        let (label, command, on_failure) = command_at(&steps, 0);
        assert_eq!(label, "Fetch latest base");
        assert_eq!(
            command,
            "if ! git fetch origin main; then git -c 'url.https://github.com/.insteadOf=git@github.com:' fetch origin main; fi"
        );
        assert!(matches!(on_failure, FailureStrategy::HandoffToAi { .. }));

        let (_, command, _) = command_at(&steps, 1);
        assert_eq!(
            command,
            r#"base_commit=$(git merge-base origin/main HEAD) && git log --oneline "$base_commit"..HEAD"#
        );
        assert!(!command.contains("git log --oneline main"));

        let (_, command, _) = command_at(&steps, 2);
        assert_eq!(
            command,
            r#"base_commit=$(git merge-base origin/main HEAD) && git diff "$base_commit"..HEAD --stat"#
        );
        assert!(!command.contains("git diff main"));

        let (label, command, _) = command_at(&steps, 3);
        assert_eq!(label, "Push to remote");
        assert_eq!(
            command,
            "if ! git push -u origin feature-branch-2:feature-branch; then git -c 'url.https://github.com/.insteadOf=git@github.com:' push -u origin feature-branch-2:feature-branch; fi"
        );

        let (label, prompt) = ai_prompt_at(&steps, 4);
        assert_eq!(label, "Create PR");
        assert!(prompt.contains("local `main` branch may be stale"));
        assert!(prompt.contains("$(git merge-base origin/main HEAD)..HEAD"));
        assert!(prompt.contains("gh pr create --base main --head feature-branch "));
    }

    #[test]
    fn rebase_pipeline_uses_signoff() {
        let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();

        let (label, command, _) = command_at(&steps, 0);
        assert_eq!(label, "Fetch latest base");
        assert_eq!(
            command,
            "if ! git fetch origin main; then git -c 'url.https://github.com/.insteadOf=git@github.com:' fetch origin main; fi"
        );

        let (label, command, _) = command_at(&steps, 1);
        assert_eq!(label, "Rebase onto base");
        assert_eq!(
            command,
            git::rebase_preserving_merge_edits_command("origin/main")
        );
        assert!(command.contains("--signoff"));
        assert!(command.contains("onto='origin/main'"));
    }

    /// The rebase step must never be a bare `git rebase`: that is exactly the
    /// command that silently drops hand edits carried by merge commits. The
    /// step also travels to remote workspaces as a single argv element, so the
    /// command must be one line.
    #[test]
    fn rebase_pipeline_routes_through_the_merge_edit_safeguard() {
        for target in ["main", "feature-branch"] {
            let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", target).unwrap();
            let (_, command, on_failure) = command_at(&steps, 1);
            assert!(command.contains("git merge-tree --write-tree"), "{command}");
            assert!(!command.contains("--rebase-merges"), "{command}");
            assert!(!command.starts_with("git rebase"), "{command}");
            assert!(!command.contains('\n'), "{command}");

            let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
                panic!("expected rebase step to have HandoffToAi failure");
            };
            assert!(prompt_template.contains(git::HAND_EDIT_COMMIT_SUBJECT_PREFIX));
            assert!(prompt_template.contains("never drop or skip them"));
        }
    }

    /// After a conflict the only correct move is `git rebase --continue`. The
    /// prompt must not send the agent back to the generated command (a long
    /// script it cannot reproduce byte for byte) and must leave aborting to
    /// the user.
    #[test]
    fn rebase_handoff_prompt_continues_never_restarts_and_leaves_abort_to_the_user() {
        let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();
        let (_, _, on_failure) = command_at(&steps, 1);
        let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
            panic!("expected rebase step to have HandoffToAi failure");
        };
        assert!(
            prompt_template.contains("run `git rebase --continue`"),
            "{prompt_template}"
        );
        assert!(
            prompt_template.contains("Never restart the rebase and never run a plain `git rebase`"),
            "{prompt_template}"
        );
        assert!(
            prompt_template.contains("`git rebase --abort` is the user's decision, not yours"),
            "{prompt_template}"
        );
        assert!(
            prompt_template.contains("refused before it started"),
            "{prompt_template}"
        );
        assert!(
            !prompt_template.contains("re-run the exact rebase command"),
            "{prompt_template}"
        );
        assert!(
            !prompt_template.contains("if you must restart"),
            "{prompt_template}"
        );
    }

    #[test]
    fn rebase_pipeline_targets_origin_branch_when_target_differs() {
        let steps =
            build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "feature-branch").unwrap();

        let (label, command, _) = command_at(&steps, 0);
        assert_eq!(label, "Fetch origin/feature-branch");
        assert_eq!(
            command,
            "if ! git fetch origin feature-branch; then git -c 'url.https://github.com/.insteadOf=git@github.com:' fetch origin feature-branch; fi"
        );

        let (label, command, _) = command_at(&steps, 1);
        assert_eq!(label, "Rebase onto origin/feature-branch");
        assert_eq!(
            command,
            git::rebase_preserving_merge_edits_command("origin/feature-branch")
        );
        assert!(command.contains("onto='origin/feature-branch'"));
    }

    #[test]
    fn rebase_pipeline_prompt_mentions_base_when_target_differs() {
        let steps =
            build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "feature-branch").unwrap();

        let fetch_failure = match &steps[0] {
            PipelineStep::Command {
                on_failure: FailureStrategy::HandoffToAi { prompt_template },
                ..
            } => prompt_template.clone(),
            _ => panic!("expected fetch step to have HandoffToAi failure"),
        };
        assert!(fetch_failure.contains("origin/feature-branch"));
        assert!(fetch_failure.contains("origin/main"));
        assert!(fetch_failure.contains("the rebase target may be wrong"));

        let rebase_failure = match &steps[1] {
            PipelineStep::Command {
                on_failure: FailureStrategy::HandoffToAi { prompt_template },
                ..
            } => prompt_template.clone(),
            _ => panic!("expected rebase step to have HandoffToAi failure"),
        };
        assert!(rebase_failure.contains("origin/feature-branch"));
        assert!(rebase_failure.contains("origin/main"));
        assert!(rebase_failure.contains("the rebase target may be wrong"));
    }

    #[test]
    fn rebase_pipeline_prompt_omits_target_note_when_target_matches_base() {
        let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();

        for step in &steps {
            if let PipelineStep::Command {
                on_failure: FailureStrategy::HandoffToAi { prompt_template },
                ..
            } = step
            {
                assert!(!prompt_template.contains("the rebase target may be wrong"));
                assert!(!prompt_template.contains("is different"));
            }
        }
    }

    #[test]
    fn squash_pipeline_prompt_requires_signoff() {
        let steps = build_commit_pipeline_steps(&PipelineKind::Squash, "main", "main").unwrap();

        let (_, prompt) = ai_prompt_at(&steps, 3);
        assert!(prompt.contains("Use the user's global git identity"));
        assert!(prompt.contains("git commit --signoff"));
    }
}
