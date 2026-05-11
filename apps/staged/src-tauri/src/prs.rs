use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

use crate::git;
use crate::session_runner;
use crate::store::{self, FailureStrategy, PipelineExecution, PipelineKind, PipelineStep, Store};

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

fn resolve_branch_repo_and_subpath(
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
struct PrStatusEvent {
    branch_id: String,
    pr_state: String,
    pr_checks_status: String,
    pr_review_decision: Option<String>,
    pr_mergeable: bool,
    pr_draft: bool,
    pr_head_sha: Option<String>,
    pr_fetched_at: i64,
    failed_checks: Vec<git::FailedCheck>,
}

// =============================================================================
// Pipeline session helper
// =============================================================================

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
    session_runner::emit_session_running(
        app_handle,
        &session.id,
        &ctx.branch.id,
        &ctx.branch.project_id,
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

fn commit_pipeline_prompt(kind: &PipelineKind) -> &'static str {
    match kind {
        PipelineKind::Rebase => "Rebase branch",
        PipelineKind::Squash => "Squash commits",
    }
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

fn build_commit_pipeline_steps(kind: &PipelineKind, base_branch: &str) -> Vec<PipelineStep> {
    match kind {
        PipelineKind::Rebase => vec![
            PipelineStep::Command {
                label: "Fetch latest base".to_string(),
                command: git_fetch_with_fallback(base_branch),
                on_failure: FailureStrategy::HandoffToAi {
                    prompt_template: format!(
                        "The fetch failed. Diagnose and fix the issue, then rebase this branch onto `origin/{base_branch}` with DCO signoffs. Resolve conflicts if present and continue the rebase. Do not push the branch.\n\n{{step_outputs}}"
                    ),
                },
            },
            PipelineStep::Command {
                label: "Rebase onto base".to_string(),
                command: format!("git rebase --signoff origin/{base_branch}"),
                on_failure: FailureStrategy::HandoffToAi {
                    prompt_template: format!(
                        "The rebase failed. Inspect the output, recover from the actual failure, resolve conflicts if present, then continue the rebase onto `origin/{base_branch}` with DCO signoffs. Do not push the branch.\n\n{{step_outputs}}"
                    ),
                },
            },
        ],
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
    }
}

async fn start_running_commit_pipeline_for_branch(
    ctx: BranchPipelineContext,
    kind: PipelineKind,
    steps: Vec<PipelineStep>,
    provider: Option<String>,
    store: Arc<Store>,
    app_handle: &tauri::AppHandle,
    registry: &Arc<session_runner::SessionRegistry>,
) -> Result<String, String> {
    let prompt = commit_pipeline_prompt(&kind);
    let pipeline = PipelineExecution::from_steps(&steps).with_kind(kind);

    let mut session = store::Session::new_running(prompt, &ctx.working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    store.create_session(&session).map_err(|e| e.to_string())?;

    let commit = store::Commit::new_pending(&ctx.branch.id).with_session(&session.id);
    store.create_commit(&commit).map_err(|e| e.to_string())?;

    session_runner::emit_session_running(
        app_handle,
        &session.id,
        &ctx.branch.id,
        &ctx.branch.project_id,
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
            provider,
            workspace_name: ctx.workspace_name,
            remote_working_dir: ctx.remote_working_dir,
        },
        store,
        app_handle.clone(),
        Arc::clone(registry),
    )?;

    Ok(session.id)
}

#[allow(clippy::too_many_arguments)]
async fn start_or_queue_commit_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    kind: PipelineKind,
    provider: Option<String>,
    target: Option<String>,
) -> Result<String, String> {
    let prompt = commit_pipeline_prompt(&kind);

    let branch_has_running_session = store
        .has_running_session_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;
    let branch_has_queued_session = !store
        .get_queued_sessions_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .is_empty();

    if branch_has_running_session || branch_has_queued_session {
        let branch = store
            .get_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
        let rebase_ref = rebase_ref_for_target(&branch, target.as_deref());
        let steps = build_commit_pipeline_steps(&kind, &rebase_ref);
        let pipeline = PipelineExecution::from_steps(&steps).with_kind(kind);
        let mut session = store::Session::new_queued(prompt);
        if let Some(ref p) = provider {
            session = session.with_provider(p);
        }
        session.pipeline = Some(pipeline);
        store.create_session(&session).map_err(|e| e.to_string())?;

        let commit = store::Commit::new_pending(&branch_id).with_session(&session.id);
        store.create_commit(&commit).map_err(|e| e.to_string())?;

        return Ok(session.id);
    }

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let rebase_ref = rebase_ref_for_target(&ctx.branch, target.as_deref());
    let steps = build_commit_pipeline_steps(&kind, &rebase_ref);

    start_running_commit_pipeline_for_branch(
        ctx,
        kind,
        steps,
        provider,
        store,
        &app_handle,
        &registry,
    )
    .await
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

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let base_branch = base_branch_name(&ctx.branch);
    let steps = build_commit_pipeline_steps(&kind, base_branch);
    let prompt = commit_pipeline_prompt(&kind);
    let pipeline = PipelineExecution::from_steps(&steps).with_kind(kind);
    let effective_provider = session.provider.clone().or(provider);

    let transitioned = store
        .transition_to_running(&session.id)
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

    session_runner::emit_session_running(
        &app_handle,
        &session.id,
        &ctx.branch.id,
        &ctx.branch.project_id,
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
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(true)
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

    app_handle
        .emit(
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
        )
        .map_err(|e| format!("Failed to emit event: {}", e))?;

    Ok(())
}

/// Refresh PR status for all branches in a project.
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_all_pr_statuses(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    project_id: String,
) -> Result<u32, String> {
    let store = get_store(&store)?;
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let branches = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?;
    let branches_with_prs: Vec<_> = branches
        .into_iter()
        .filter(|b| b.pr_number.is_some())
        .collect();

    let mut refreshed_count = 0u32;

    for branch in branches_with_prs {
        let pr_number = branch.pr_number.unwrap();
        let github_repo = match resolve_branch_repo_and_subpath(&store, &project, &branch) {
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

        let pr_result = {
            let github_repo = github_repo.clone();
            tauri::async_runtime::spawn_blocking(move || {
                git::fetch_pr_status_for_repo(&github_repo, pr_number)
            })
            .await
            .map_err(|e| format!("refresh_all_pr_statuses task failed: {e}"))?
        };
        match pr_result {
            Ok(pr_status) => {
                let mergeable = pr_status.mergeable == "MERGEABLE";
                let pr_fetched_at = store::now_timestamp();

                if let Err(e) = store.update_branch_pr_status(
                    &branch.id,
                    Some(pr_status.state.clone()),
                    Some(pr_status.checks_summary.state.clone()),
                    pr_status.review_decision.clone(),
                    Some(mergeable),
                    Some(pr_status.is_draft),
                    None,
                    None,
                    pr_status.head_sha.clone(),
                ) {
                    log::warn!("Failed to update PR status for branch {}: {}", branch.id, e);
                    continue;
                }

                refreshed_count += 1;

                if let Err(e) = app_handle.emit(
                    "pr-status-changed",
                    PrStatusEvent {
                        branch_id: branch.id.clone(),
                        pr_state: pr_status.state,
                        pr_checks_status: pr_status.checks_summary.state,
                        pr_review_decision: pr_status.review_decision,
                        pr_mergeable: mergeable,
                        pr_draft: pr_status.is_draft,
                        pr_head_sha: pr_status.head_sha,
                        pr_fetched_at,
                        failed_checks: pr_status.failed_checks,
                    },
                ) {
                    log::warn!("Failed to emit pr-status-changed event: {}", e);
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to fetch PR status for branch {} (PR #{}): {}",
                    branch.id,
                    pr_number,
                    e
                );
            }
        }
    }

    app_handle
        .emit("pr-statuses-refreshed", &project_id)
        .map_err(|e| format!("Failed to emit event: {}", e))?;

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

    app_handle
        .emit("pr-status-cleared", &branch_id)
        .map_err(|e| format!("Failed to emit event: {}", e))?;

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

    git::has_unpushed_commits(Path::new(&workdir.path), &branch.branch_name)
        .map_err(|e| e.to_string())
}

/// Push a branch to its remote by kicking off an agent session.
#[tauri::command(rename_all = "camelCase")]
pub async fn push_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    force: Option<bool>,
) -> Result<String, String> {
    let store = get_store(&store)?;
    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;

    let force = force.unwrap_or(false);

    let push_command = if force {
        git_push_with_fallback(&format!(
            "-u origin {} --force-with-lease",
            ctx.branch.branch_name
        ))
    } else {
        git_push_with_fallback(&format!("-u origin {}", ctx.branch.branch_name))
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

    let steps = vec![PipelineStep::Command {
        label: "Push to remote".to_string(),
        command: push_command,
        on_failure,
    }];

    let prompt = if force {
        "Force push the current branch to the remote".to_string()
    } else {
        "Push the current branch to the remote with a normal push. If the push fails for a recoverable reason, diagnose and fix it, then retry with a normal push. Do not force push.".to_string()
    };

    start_pipeline_for_branch(
        ctx,
        steps,
        &prompt,
        "push",
        provider,
        store,
        &app_handle,
        &registry,
    )
}

/// Rebase a branch via a pipeline.
///
/// When `target` is `None` or `"base"`, rebases onto `origin/{base_branch}`
/// (the default behaviour used by the base-moved row and the `…` menu).
/// When `target` is `"origin"`, rebases onto `origin/{branch_name}` so that
/// the local branch incorporates remote-only commits (used by the diverged row).
#[tauri::command(rename_all = "camelCase")]
pub async fn rebase_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    target: Option<String>,
) -> Result<String, String> {
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
#[tauri::command(rename_all = "camelCase")]
pub async fn squash_commits(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<String, String> {
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
        let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main");

        let (_, command, _) = command_at(&steps, 0);
        assert_eq!(
            command,
            "if ! git fetch origin main; then git -c 'url.https://github.com/.insteadOf=git@github.com:' fetch origin main; fi"
        );

        let (_, command, _) = command_at(&steps, 1);
        assert_eq!(command, "git rebase --signoff origin/main");
    }

    #[test]
    fn squash_pipeline_prompt_requires_signoff() {
        let steps = build_commit_pipeline_steps(&PipelineKind::Squash, "main");

        let (_, prompt) = ai_prompt_at(&steps, 3);
        assert!(prompt.contains("Use the user's global git identity"));
        assert!(prompt.contains("git commit --signoff"));
    }
}
