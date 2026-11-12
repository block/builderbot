//! Creating a pull request: fetch the base, gather context, push, and hand
//! off to an agent that writes the title and body and runs `gh pr create`.

use std::sync::Arc;

use crate::git;
use crate::session_runner;
use crate::store::{FailureStrategy, PipelineStep, Store};

use super::context::{base_branch_name, resolve_branch_pipeline_context};
use super::git_commands::{git_fetch_with_fallback, git_push_with_fallback, push_refspec};
use super::pipeline::start_pipeline_for_branch;

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

pub(super) fn build_create_pr_pipeline_steps(
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
