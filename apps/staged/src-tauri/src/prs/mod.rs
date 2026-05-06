//! Branch git actions and pull requests: the PR button and the git rows of
//! the branch card, end to end on the backend.
//!
//! Every action here is a *pipeline session* on the branch — a `sessions` row
//! carrying a [`store::PipelineExecution`] that the session runner steps
//! through, handing off to an agent only where a step says so — and every
//! action queues behind the branch's in-flight work instead of failing, so a
//! second click never races the first.
//!
//! - [`context`] resolves a branch to the repository and working directory
//!   its pipelines run in.
//! - [`pipeline`] is the session-row plumbing the actions share: the
//!   start-or-queue response, the prompts that label the rows, the
//!   busy and dedupe checks, and the run-now row and its launch.
//! - [`git_commands`] spells the fetch and push shell commands, with their
//!   HTTPS fallback.
//! - [`commit`] is rebase and squash: the pipelines that produce a commit,
//!   and the force push "Rebase and force push" queues behind its rebase.
//! - [`push`] is the push pipeline and its variants ([`PushVariant`]).
//! - [`pull`] fast-forwards the branch to origin, now or queued.
//! - [`revert`] reverts a completed commit with a new signed-off commit, run
//!   now and refused while the branch is busy.
//! - [`git_pipeline`] rebuilds a queued push or pull when it is dequeued.
//! - [`deferred`] is "Push after current work": a push armed on the branch
//!   row that fires once the branch and its project are idle.
//! - [`create_pr`] pushes and opens a pull request through an agent.
//! - [`pr_status`] fetches, caches and clears GitHub PR status.
//! - [`commands`] holds the Tauri command wrappers; the web server reaches
//!   the same `*_impl` / `*_for_branch` entry points directly.

pub(crate) mod commands;
mod commit;
mod context;
mod create_pr;
mod deferred;
mod git_commands;
mod git_pipeline;
mod pipeline;
mod pr_status;
mod pull;
mod push;
mod revert;
#[cfg(test)]
mod tests;

pub(crate) use commit::{
    start_or_queue_commit_pipeline_for_branch, start_queued_commit_pipeline_for_branch,
};
pub(crate) use context::{resolve_branch_pipeline_context, resolve_branch_repo_and_subpath};
pub(crate) use create_pr::start_create_pr_pipeline_for_branch;
pub use deferred::DeferredPushResponse;
pub(crate) use deferred::{
    cancel_deferred_push_for_branch, defer_branch_push_for_branch, fire_deferred_push_if_idle,
};
pub(crate) use git_pipeline::start_queued_git_pipeline_for_branch;
pub use pipeline::BranchPipelineResponse;
pub(crate) use pipeline::{branch_work_in_flight, BranchWorkInFlight};
pub(crate) use pr_status::{
    clear_branch_pr_status_impl, refresh_pr_status_impl, refresh_project_pr_statuses,
    PrStatusEvent, PR_REFRESH_CONCURRENCY,
};
pub(crate) use pull::pull_or_queue_branch_for_branch;
pub(crate) use revert::revert_commit_for_branch;
#[cfg(test)]
pub(crate) use push::build_push_pipeline_steps;
pub(crate) use push::{
    has_unpushed_commits_impl, start_or_queue_push_pipeline_for_branch, PushVariant,
};
