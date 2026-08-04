use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::git;
use crate::session_commands::BranchSessionLaunchStatus;
use crate::session_runner;
use crate::store::{self, FailureStrategy, PipelineExecution, PipelineKind, PipelineStep, Store};

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
}

impl BranchPipelineResponse {
    fn running(session_id: String) -> Self {
        Self {
            session_id,
            session_status: BranchSessionLaunchStatus::Running,
        }
    }

    fn queued(session_id: String) -> Self {
        Self {
            session_id,
            session_status: BranchSessionLaunchStatus::Queued,
        }
    }
}

/// Resolved context for a branch, ready to start a pipeline session.
struct BranchPipelineContext {
    branch: store::Branch,
    working_dir: PathBuf,
    workspace_name: Option<String>,
    remote_working_dir: Option<PathBuf>,
}

/// Resolve branch, project, and working directory for a pipeline command.
///
/// All pipeline commands (create_pr, push_branch, rebase_branch, squash_commits)
/// share the same setup: look up the branch and project, resolve the working
/// directory (local worktree vs remote clone), and compute the remote working
/// directory for workspace-based branches.
fn resolve_branch_pipeline_context(
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
    let pipeline = PipelineExecution::from_steps(&steps);

    let mut session = store::Session::new_running(prompt, &ctx.working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
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
/// - `Some("origin")` → the branch's own name (e.g. `feature-xyz`)
fn rebase_ref_for_target(branch: &store::Branch, target: Option<&str>) -> String {
    match target {
        Some("origin") => branch.branch_name.clone(),
        _ => base_branch_name(branch).to_string(),
    }
}

const PUSH_PROMPT: &str = "Push the current branch to the remote with a normal push. If the push fails for a recoverable reason, diagnose and fix it, then retry with a normal push. Do not force push.";
const FORCE_PUSH_PROMPT: &str = "Force push the current branch to the remote";
/// A pull never hands off to an agent (every step aborts), so this is purely the
/// session's label.
const PULL_PROMPT: &str = "Pull from origin";

/// Prompt for a pipeline session, which doubles as its timeline label.
///
/// `push_force` only matters for [`PipelineKind::Push`]; the other kinds ignore
/// it. Queued and running paths share this so a session's label doesn't change
/// when it is drained.
fn pipeline_prompt(kind: &PipelineKind, push_force: bool) -> &'static str {
    match kind {
        PipelineKind::Rebase => "Rebase branch",
        PipelineKind::Squash => "Squash commits",
        PipelineKind::Push if push_force => FORCE_PUSH_PROMPT,
        PipelineKind::Push => PUSH_PROMPT,
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
/// the branch's own name for "rebase onto origin" (used when local has
/// diverged from `origin/{branch}`). Only the rebase variant consults this
/// value; squash always operates against the base branch.
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
                    command: format!("git rebase --signoff origin/{rebase_target}"),
                    on_failure: FailureStrategy::HandoffToAi {
                        prompt_template: format!(
                            "The rebase failed. Inspect the output, recover from the actual failure, resolve conflicts if present, then continue the rebase onto `origin/{rebase_target}` with DCO signoffs. Do not push the branch.{target_note}\n\n{{step_outputs}}"
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
/// Rebuilt from the persisted `push_force` flag on dequeue rather than replayed
/// from the queued pipeline, so a queued push always pushes the branch's current
/// name with the command the user asked for.
fn build_push_pipeline_steps(branch_name: &str, force: bool) -> Vec<PipelineStep> {
    let push_command = if force {
        git_push_with_fallback(&format!("-u origin {branch_name} --force-with-lease"))
    } else {
        git_push_with_fallback(&format!("-u origin {branch_name}"))
    };

    let on_failure = if force {
        FailureStrategy::HandoffToAi {
            prompt_template:
                "The force push failed. Diagnose and fix the issue, then retry the force push.\n\n{step_outputs}"
                    .to_string(),
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
/// Rebuilt from the branch's current name on dequeue, like the push steps.
fn build_pull_pipeline_steps(branch_name: &str) -> Vec<PipelineStep> {
    vec![
        PipelineStep::Command {
            label: format!("Fetch origin/{branch_name}"),
            command: git_fetch_with_fallback(branch_name),
            on_failure: FailureStrategy::Abort { marker: None },
        },
        PipelineStep::Command {
            label: format!("Fast-forward to origin/{branch_name}"),
            command: format!("git merge --ff-only origin/{branch_name}"),
            on_failure: FailureStrategy::Abort { marker: None },
        },
    ]
}

/// The rows a run-now pipeline needs before the session runner can pick it up.
///
/// Inserted under the branch launch lock and consumed by
/// [`launch_running_pipeline_session`] once the lock is released.
struct RunningPipelineSession {
    session_id: String,
    pipeline: PipelineExecution,
    prompt: &'static str,
}

/// Insert the session and pending-commit rows for a rebase/squash that runs now.
///
/// Callers must hold the branch launch lock: these rows are what make the branch
/// look busy to a concurrent launch, so the busy check and this insert have to be
/// atomic. Synchronous for the same reason — the lock must not be held across an
/// await.
fn insert_running_commit_pipeline_session(
    store: &Arc<Store>,
    ctx: &BranchPipelineContext,
    kind: PipelineKind,
    steps: &[PipelineStep],
    rebase_target: Option<String>,
    provider: Option<&str>,
) -> Result<RunningPipelineSession, String> {
    let prompt = pipeline_prompt(&kind, false);
    let mut pipeline = PipelineExecution::from_steps(steps).with_kind(kind);
    if let Some(target) = rebase_target {
        pipeline = pipeline.with_rebase_target(target);
    }

    let mut session = store::Session::new_running(prompt, &ctx.working_dir);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    store.create_session(&session).map_err(|e| e.to_string())?;

    let commit = store::Commit::new_pending(&ctx.branch.id).with_session(&session.id);
    store.create_commit(&commit).map_err(|e| e.to_string())?;

    Ok(RunningPipelineSession {
        session_id: session.id,
        pipeline,
        prompt,
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

    session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: running.session_id.clone(),
            prompt: running.prompt.to_string(),
            steps,
            pipeline: running.pipeline,
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

    Ok(running.session_id)
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
fn queue_commit_pipeline_if_branch_busy(
    store: &Arc<Store>,
    branch_id: &str,
    kind: &PipelineKind,
    provider: Option<&str>,
    target: Option<&str>,
) -> Result<Option<String>, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();
    queue_commit_pipeline_locked(store, branch_id, kind, provider, target)
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
    if !branch_has_work_in_flight(store, branch_id)? {
        return Ok(None);
    }

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let base_branch = base_branch_name(&branch);
    let rebase_ref = rebase_ref_for_target(&branch, target);
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
    let mut session = store::Session::new_queued(pipeline_prompt(kind, false));
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
) -> Result<BranchPipelineResponse, String> {
    // Pre-flight check: a branch that is already busy queues without resolving a
    // pipeline context, which for a remote branch can depend on a running
    // workspace the queued work will only need later.
    if let Some(session_id) = queue_commit_pipeline_if_branch_busy(
        &store,
        &branch_id,
        &kind,
        provider.as_deref(),
        target.as_deref(),
    )? {
        return Ok(BranchPipelineResponse::queued(session_id));
    }

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let base_branch = base_branch_name(&ctx.branch).to_string();
    let rebase_ref = rebase_ref_for_target(&ctx.branch, target.as_deref());
    let steps = build_commit_pipeline_steps(&kind, &base_branch, &rebase_ref)?;
    let rebase_target = persisted_rebase_target(target.as_deref(), &rebase_ref);

    // Re-check and insert under the lock, mirroring
    // `session_commands::start_or_queue_branch_session_for_store`: the pre-flight
    // check released the lock to resolve the context above, so without a second
    // look two near-simultaneous actions could both have seen an idle branch and
    // both start running — exactly what git-pipeline exclusivity exists to stop.
    let running = {
        let launch_lock = crate::session_commands::branch_session_launch_lock_for(&branch_id);
        let _guard = launch_lock.lock().unwrap();

        if let Some(session_id) = queue_commit_pipeline_locked(
            &store,
            &branch_id,
            &kind,
            provider.as_deref(),
            target.as_deref(),
        )? {
            return Ok(BranchPipelineResponse::queued(session_id));
        }

        insert_running_commit_pipeline_session(
            &store,
            &ctx,
            kind,
            &steps,
            rebase_target,
            provider.as_deref(),
        )?
    };

    let session_id = launch_running_pipeline_session(
        ctx,
        running,
        steps,
        "commit",
        provider,
        store,
        &app_handle,
        &registry,
    )?;

    Ok(BranchPipelineResponse::running(session_id))
}

pub(crate) async fn start_queued_commit_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    session: store::Session,
    provider: Option<String>,
) -> Result<bool, String> {
    let kind = session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.kind.clone())
        .ok_or_else(|| format!("Queued session {} has no pipeline kind", session.id))?;
    let queued_rebase_target = session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.rebase_target.clone());

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let base_branch = base_branch_name(&ctx.branch).to_string();
    let rebase_ref = queued_rebase_target
        .clone()
        .unwrap_or_else(|| base_branch.clone());
    let steps = build_commit_pipeline_steps(&kind, &base_branch, &rebase_ref)?;
    let prompt = pipeline_prompt(&kind, false);
    let mut pipeline = PipelineExecution::from_steps(&steps).with_kind(kind);
    if let Some(target) = queued_rebase_target {
        pipeline = pipeline.with_rebase_target(target);
    }
    let effective_provider = session.provider.clone().or(provider);

    let transitioned = store
        .transition_queued_to_running(&session.id)
        .map_err(|e| e.to_string())?;
    if !transitioned {
        return Ok(false);
    }

    store
        .mark_session_artifact_started(&session.id)
        .map_err(|e| e.to_string())?;

    store
        .prepare_queued_session(&session.id, &ctx.working_dir.to_string_lossy(), prompt)
        .map_err(|e| e.to_string())?;
    store
        .update_session_pipeline(&session.id, &pipeline)
        .map_err(|e| e.to_string())?;

    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

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
    )?;

    Ok(true)
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
    ctx: &BranchPipelineContext,
    force: bool,
    steps: &[PipelineStep],
    provider: Option<&str>,
) -> Result<RunningPipelineSession, String> {
    let prompt = pipeline_prompt(&PipelineKind::Push, force);
    let pipeline = PipelineExecution::from_steps(steps)
        .with_kind(PipelineKind::Push)
        .with_push_force(force);

    let mut session =
        store::Session::new_running(prompt, &ctx.working_dir).with_branch(&ctx.branch.id);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(RunningPipelineSession {
        session_id: session.id,
        pipeline,
        prompt,
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
/// synchronous because the lock must not be held across an await. A push and a
/// force push dedupe separately — they are different operations, so a queued
/// normal push must not swallow a force push request.
fn queue_push_pipeline_if_branch_busy(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    force: bool,
) -> Result<Option<String>, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();
    queue_push_pipeline_locked(store, branch_id, provider, force)
}

/// The body of [`queue_push_pipeline_if_branch_busy`], for the run-now path,
/// which re-checks while already holding the branch launch lock.
fn queue_push_pipeline_locked(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    force: bool,
) -> Result<Option<String>, String> {
    if !branch_has_work_in_flight(store, branch_id)? {
        return Ok(None);
    }

    if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
        pipeline.kind.as_ref() == Some(&PipelineKind::Push) && pipeline.push_force == force
    })? {
        return Ok(Some(existing));
    }

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let steps = build_push_pipeline_steps(&branch.branch_name, force);
    let pipeline = PipelineExecution::from_steps(&steps)
        .with_kind(PipelineKind::Push)
        .with_push_force(force);
    // No artifact row: a push produces no commit, and a pending-commit stub
    // would render as a failed commit once the push finishes without a new sha.
    // `branch_id` is what keeps this session on the branch queue instead.
    let mut session = store::Session::new_queued(pipeline_prompt(&PipelineKind::Push, force))
        .with_branch(branch_id);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline);
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(Some(session.id))
}

/// Start a queued push or pull pipeline that reached the front of the branch
/// queue.
///
/// Steps are rebuilt from the branch's current name and the persisted
/// `push_force` flag rather than replayed from the queued pipeline, matching
/// [`start_queued_commit_pipeline_for_branch`] so a branch renamed while the work
/// waited still acts on the right ref.
pub(crate) async fn start_queued_git_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    session: store::Session,
    provider: Option<String>,
) -> Result<bool, String> {
    let queued_pipeline = session
        .pipeline
        .as_ref()
        .ok_or_else(|| format!("Queued session {} has no pipeline", session.id))?;
    let kind = queued_pipeline
        .kind
        .clone()
        .ok_or_else(|| format!("Queued session {} has no pipeline kind", session.id))?;
    let force = queued_pipeline.push_force;

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let (steps, session_type) = match kind {
        PipelineKind::Push => (
            build_push_pipeline_steps(&ctx.branch.branch_name, force),
            "push",
        ),
        PipelineKind::Pull => (build_pull_pipeline_steps(&ctx.branch.branch_name), "pull"),
        PipelineKind::Rebase | PipelineKind::Squash => {
            return Err(format!(
                "Queued git pipeline session {} has non-git kind {kind:?}",
                session.id
            ));
        }
    };
    let prompt = pipeline_prompt(&kind, force);
    let pipeline = PipelineExecution::from_steps(&steps)
        .with_kind(kind)
        .with_push_force(force);
    let effective_provider = session.provider.clone().or(provider);

    let transitioned = store
        .transition_queued_to_running(&session.id)
        .map_err(|e| e.to_string())?;
    if !transitioned {
        return Ok(false);
    }

    // No `mark_session_artifact_started` call: a git pipeline has no queued
    // artifact stub whose timestamp needs restamping when the work actually
    // starts.
    store
        .prepare_queued_session(&session.id, &ctx.working_dir.to_string_lossy(), prompt)
        .map_err(|e| e.to_string())?;
    store
        .update_session_pipeline(&session.id, &pipeline)
        .map_err(|e| e.to_string())?;

    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

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
    )?;

    Ok(true)
}

/// What the branch queue decided to do with a pull request.
enum PullDisposition {
    /// Waiting its turn: either a freshly queued session, or the queued pull that
    /// already covers this request.
    Queued(String),
    /// The branch was idle, so the pull runs now. The id is the session that marks
    /// the branch busy for its duration.
    RunningNow(String),
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
    branch_id: &str,
) -> Result<PullDisposition, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();

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
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(if busy {
        PullDisposition::Queued(session.id)
    } else {
        PullDisposition::RunningNow(session.id)
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
    let session_id = match claim_or_queue_pull_for_branch(&store, &branch_id)? {
        PullDisposition::Queued(session_id) => return Ok(Some(session_id)),
        PullDisposition::RunningNow(session_id) => session_id,
    };

    let pull_store = Arc::clone(&store);
    let pull_branch_id = branch_id.clone();
    let pulled = tauri::async_runtime::spawn_blocking(move || {
        crate::timeline::pull_branch_ff_only_impl(&pull_store, &pull_branch_id)
    })
    .await
    .map_err(|e| format!("Pull task failed: {e}"))?;

    finish_immediate_pull_session(
        &store,
        &session_id,
        pulled.as_ref().err().map(String::as_str),
    );

    // Anything the user requested while the pull held the branch is queued behind
    // the marker session, and the marker bypassed the session runner that would
    // normally drain it. Spawned rather than awaited so the pull's own result
    // isn't held up by starting someone else's work.
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::session_commands::drain_queued_sessions_for_branch(
            store,
            registry,
            app_handle,
            branch_id.clone(),
            None,
        )
        .await
        {
            log::warn!("[prs] Failed to drain queued sessions after pulling {branch_id}: {e}");
        }
    });

    pulled.map(|()| None)
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
- Run `gh pr create --base {base_branch} --title <title> --body <body>{draft_flag}` to create the {pr_type}.
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
    branch_name: &str,
) -> Vec<PipelineStep> {
    let base_ref = git::origin_ref_for_branch(base_branch);

    vec![
        PipelineStep::Command {
            label: "Fetch latest base".to_string(),
            command: git_fetch_with_fallback(base_branch),
            on_failure: FailureStrategy::HandoffToAi {
                prompt_template: create_pr_handoff_prompt(
                    pr_type,
                    base_branch,
                    &base_ref,
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
            command: git_push_with_fallback(&format!("-u origin {branch_name}")),
            on_failure: FailureStrategy::HandoffToAi {
                prompt_template: create_pr_handoff_prompt(
                    pr_type,
                    base_branch,
                    &base_ref,
                    draft_flag,
                    &format!(
                        "The push failed while creating a {pr_type}. Diagnose and fix the issue, then retry the push. After the push succeeds, create or recover the {pr_type}."
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
    let steps =
        build_create_pr_pipeline_steps(pr_type, base_branch, draft_flag, &ctx.branch.branch_name);

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
        .update_branch_pr_number(&branch_id, pr_number)
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

    store
        .update_branch_pr_status(
            &branch_id,
            Some(pr_status.state.clone()),
            Some(pr_status.checks_summary.state.clone()),
            pr_status.review_decision.clone(),
            Some(mergeable),
            Some(pr_status.is_draft),
            None,
            None,
            pr_status.head_sha.clone(),
        )
        .map_err(|e| e.to_string())?;

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

                    if let Err(e) = store.update_branch_pr_status(
                        &branch_id,
                        Some(pr_status.state.clone()),
                        Some(pr_status.checks_summary.state.clone()),
                        pr_status.review_decision.clone(),
                        Some(mergeable),
                        Some(pr_status.is_draft),
                        None,
                        None,
                        pr_status.head_sha.clone(),
                    ) {
                        log::warn!("Failed to update PR status for branch {}: {}", branch_id, e);
                        return Ok::<bool, String>(false);
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

                    Ok(true)
                }
                Err(e) => {
                    log::warn!(
                        "Failed to fetch PR status for branch {} (PR #{}): {}",
                        branch_id,
                        pr_number,
                        e
                    );
                    Ok(false)
                }
            }
        }));
    }

    let refreshed_count = collect_branch_refresh_results(tasks).await?;

    crate::web_server::emit_to_all(app_handle, "pr-statuses-refreshed", project_id);

    Ok(refreshed_count)
}

async fn collect_branch_refresh_results<T, E>(tasks: Vec<T>) -> Result<u32, String>
where
    T: std::future::Future<Output = Result<Result<bool, String>, E>>,
    E: std::fmt::Display,
{
    let mut refreshed_count = 0u32;
    let mut failed_branch_count = 0u32;
    let mut task_errors = Vec::new();

    for task in tasks {
        match task.await {
            Ok(Ok(refreshed)) => {
                if refreshed {
                    refreshed_count += 1;
                } else {
                    failed_branch_count += 1;
                }
            }
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

    store
        .update_branch_pr_status(&branch_id, None, None, None, None, None, None, None, None)
        .map_err(|e| e.to_string())?;

    crate::web_server::emit_to_all(&app_handle, "pr-status-cleared", &branch_id);

    Ok(())
}

/// Look up an existing open PR for a branch on GitHub and persist it.
///
/// Called on component mount when `branch.prNumber` is null but the branch has
/// been pushed. Runs `gh pr view <branch>` in the background so the frontend
/// is not blocked. Returns the recovered PR number, or None if no PR exists.
#[tauri::command(rename_all = "camelCase")]
pub async fn recover_branch_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<Option<u64>, String> {
    let store = get_store(&store)?;

    recover_branch_pr_impl(store, branch_id).await
}

pub(crate) async fn recover_branch_pr_impl(
    store: Arc<Store>,
    branch_id: String,
) -> Result<Option<u64>, String> {
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    // If the branch already has a PR number, nothing to recover
    if branch.pr_number.is_some() {
        return Ok(branch.pr_number);
    }

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let is_remote = branch.branch_type == store::BranchType::Remote;
    let branch_name = branch.branch_name.clone();

    let (repo_slug, _) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;

    let working_dir = if is_remote {
        crate::paths::repos_dir()
            .map(|d| d.join(&repo_slug))
            .ok_or_else(|| "Cannot determine clone path for remote branch".to_string())?
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;
        PathBuf::from(&workdir.path)
    };

    let pr_info = tauri::async_runtime::spawn_blocking(move || {
        git::get_pr_for_branch(&working_dir, &branch_name)
    })
    .await
    .map_err(|e| format!("recover_branch_pr task failed: {e}"))?
    .map_err(|e| e.to_string())?;

    if let Some(ref info) = pr_info {
        let pr_number = info.number;
        store
            .update_branch_pr_number(&branch_id, Some(pr_number))
            .map_err(|e| e.to_string())?;
        log::info!(
            "recover_branch_pr: recovered PR #{} for branch_id={}",
            pr_number,
            branch_id
        );
        Ok(Some(pr_number))
    } else {
        Ok(None)
    }
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
        let branch_name = branch.branch_name.clone();

        // Run the blocking SSH calls on a background thread so we don't
        // block the Tauri IPC thread and freeze the UI.
        let result = tauri::async_runtime::spawn_blocking(move || {
            let remote_ref = format!("origin/{}", branch_name);
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
    let branch_name = branch.branch_name.clone();
    tauri::async_runtime::spawn_blocking(move || {
        git::has_unpushed_commits(Path::new(&path), &branch_name).map_err(|e| e.to_string())
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
    force: Option<bool>,
) -> Result<BranchPipelineResponse, String> {
    let force = force.unwrap_or(false);

    // Pre-flight check, then a re-check at insert time — see
    // `start_or_queue_commit_pipeline_for_branch` for why both are needed.
    if let Some(session_id) =
        queue_push_pipeline_if_branch_busy(&store, &branch_id, provider.as_deref(), force)?
    {
        return Ok(BranchPipelineResponse::queued(session_id));
    }

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let steps = build_push_pipeline_steps(&ctx.branch.branch_name, force);

    let running = {
        let launch_lock = crate::session_commands::branch_session_launch_lock_for(&branch_id);
        let _guard = launch_lock.lock().unwrap();

        if let Some(session_id) =
            queue_push_pipeline_locked(&store, &branch_id, provider.as_deref(), force)?
        {
            return Ok(BranchPipelineResponse::queued(session_id));
        }

        insert_running_push_pipeline_session(&store, &ctx, force, &steps, provider.as_deref())?
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
#[tauri::command(rename_all = "camelCase")]
pub async fn push_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    force: Option<bool>,
) -> Result<BranchPipelineResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_push_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
        force,
    )
    .await
}

/// Rebase a branch onto its base branch via a pipeline.
///
/// When `target` is `None` or `"base"`, rebases onto `origin/{base_branch}`
/// (the default behaviour used by the base-moved row and the `…` menu).
/// When `target` is `"origin"`, rebases onto `origin/{branch_name}` so that
/// the local branch incorporates remote-only commits (used by the diverged row).
///
/// Queues behind in-flight branch work instead of failing, so the response
/// reports whether the rebase started or is waiting on the branch queue.
#[tauri::command(rename_all = "camelCase")]
pub async fn rebase_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    target: Option<String>,
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

    fn setup_branch_store() -> (Arc<Store>, store::Branch) {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        (store, branch)
    }

    /// Make the branch busy by linking a running note session to it, which is
    /// what `has_running_session_for_branch` looks for.
    fn start_running_note_session(store: &Arc<Store>, branch_id: &str) {
        let session = store::Session::new_running("write a note", Path::new("/tmp/staged-test"));
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
        queue_commit_pipeline_if_branch_busy(store, branch_id, &kind, None, target).unwrap()
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

    fn queue_push(store: &Arc<Store>, branch_id: &str, force: bool) -> Option<String> {
        queue_push_pipeline_if_branch_busy(store, branch_id, None, force).unwrap()
    }

    #[test]
    fn idle_branch_pushes_immediately_instead_of_queueing() {
        let (store, branch) = setup_branch_store();

        assert_eq!(queue_push(&store, &branch.id, false), None);
        assert!(store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn busy_branch_queues_push_with_branch_link_and_no_artifact() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);

        let session_id = queue_push(&store, &branch.id, false)
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

        let first = queue_push(&store, &branch.id, false).unwrap();
        let second = queue_push(&store, &branch.id, false).unwrap();

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

        let push = queue_push(&store, &branch.id, false).unwrap();
        let force_push = queue_push(&store, &branch.id, true).unwrap();

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
        assert_eq!(queue_push(&store, &branch.id, true), Some(force_push));
    }

    #[test]
    fn queued_push_alone_keeps_the_branch_busy() {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        queue_push(&store, &branch.id, false).unwrap();

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

        assert!(queue_push(&store, &branch.id, false).is_some());
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
        assert!(queue_push(&store, &branch.id, false).is_some());
    }

    #[test]
    fn inserting_a_running_push_is_what_makes_the_branch_busy() {
        let (store, branch) = setup_branch_store();
        let steps = build_push_pipeline_steps(&branch.branch_name, false);

        insert_running_push_pipeline_session(
            &store,
            &pipeline_context(&branch),
            false,
            &steps,
            None,
        )
        .unwrap();

        // A push has no artifact, so `branch_id` is what the re-check keys on.
        assert!(queue_push_pipeline_locked(&store, &branch.id, None, true)
            .unwrap()
            .is_some());
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
        let normal = build_push_pipeline_steps("feature", false);
        let (label, command, on_failure) = command_at(&normal, 0);
        assert_eq!(label, "Push to remote");
        assert!(command.contains("-u origin feature"));
        assert!(!command.contains("--force-with-lease"));
        assert!(matches!(
            on_failure,
            FailureStrategy::Abort { marker } if marker.as_deref() == Some("non-fast-forward")
        ));

        // The drain path re-derives the ref, so a branch renamed while the push
        // waited pushes the new name rather than the queued one.
        let forced = build_push_pipeline_steps("feature-renamed", true);
        let (_, forced_command, forced_on_failure) = command_at(&forced, 0);
        assert!(forced_command.contains("-u origin feature-renamed --force-with-lease"));
        assert!(matches!(
            forced_on_failure,
            FailureStrategy::HandoffToAi { .. }
        ));
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
        match claim_or_queue_pull_for_branch(store, branch_id).unwrap() {
            PullDisposition::Queued(session_id) => Some(session_id),
            PullDisposition::RunningNow(_) => None,
        }
    }

    fn claim_pull(store: &Arc<Store>, branch_id: &str) -> String {
        match claim_or_queue_pull_for_branch(store, branch_id).unwrap() {
            PullDisposition::RunningNow(session_id) => session_id,
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
        assert!(queue_push(&store, &branch.id, false).is_some());
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
        let push = queue_push(&store, &branch.id, false).unwrap();

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

        assert!(queue_push(&store, &branch.id, false).is_some());
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

    #[tokio::test]
    async fn collect_branch_refresh_results_tolerates_partial_task_failure() {
        let tasks = vec![
            tokio::spawn(async { Ok::<bool, String>(true) }),
            tokio::spawn(async {
                panic!("simulated branch task panic");
                #[allow(unreachable_code)]
                Ok::<bool, String>(true)
            }),
            tokio::spawn(async { Ok::<bool, String>(false) }),
        ];

        let refreshed_count = collect_branch_refresh_results(tasks).await.unwrap();

        assert_eq!(refreshed_count, 1);
    }

    #[tokio::test]
    async fn collect_branch_refresh_results_fails_when_all_tasks_fail() {
        let tasks = vec![
            tokio::spawn(async { Err::<bool, String>("simulated semaphore failure".to_string()) }),
            tokio::spawn(async {
                panic!("simulated branch task panic");
                #[allow(unreachable_code)]
                Ok::<bool, String>(true)
            }),
        ];

        let err = collect_branch_refresh_results(tasks).await.unwrap_err();

        assert!(err.contains("all PR status refresh tasks failed"));
        assert!(err.contains("simulated semaphore failure"));
    }

    #[tokio::test]
    async fn collect_branch_refresh_results_fails_when_all_branches_fail() {
        let tasks = vec![
            tokio::spawn(async { Ok::<bool, String>(false) }),
            tokio::spawn(async { Ok::<bool, String>(false) }),
        ];

        let err = collect_branch_refresh_results(tasks).await.unwrap_err();

        assert!(err.contains("all PR status refresh tasks failed"));
        assert!(err.contains("2 branch refreshes failed"));
    }

    #[test]
    fn create_pr_pipeline_fetches_base_and_uses_origin_merge_base_for_context() {
        let steps = build_create_pr_pipeline_steps("pull request", "main", "", "feature-branch");

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
            "if ! git push -u origin feature-branch; then git -c 'url.https://github.com/.insteadOf=git@github.com:' push -u origin feature-branch; fi"
        );

        let (label, prompt) = ai_prompt_at(&steps, 4);
        assert_eq!(label, "Create PR");
        assert!(prompt.contains("local `main` branch may be stale"));
        assert!(prompt.contains("$(git merge-base origin/main HEAD)..HEAD"));
        assert!(prompt.contains("gh pr create --base main"));
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
        assert_eq!(command, "git rebase --signoff origin/main");
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
        assert_eq!(command, "git rebase --signoff origin/feature-branch");
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
