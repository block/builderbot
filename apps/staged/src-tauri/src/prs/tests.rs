//! Shared fixtures for the `prs` tests; each submodule covers one of the
//! action modules.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::commit::{
    build_commit_pipeline_steps, insert_running_commit_pipeline_session,
    queue_commit_pipeline_if_branch_busy, queue_commit_pipeline_locked, rebase_ref_for_target,
    with_follow_up_force_push_locked, with_follow_up_force_push_or_fail_rebase_locked,
};
use super::context::BranchPipelineContext;
use super::create_pr::build_create_pr_pipeline_steps;
use super::deferred::{
    cancel_deferred_push_for_branch, defer_branch_push_with, fire_deferred_push_with,
    DeferredPushResponse,
};
use super::git_pipeline::rebuild_queued_git_pipeline;
use super::pipeline::{
    branch_has_work_in_flight, pipeline_prompt, BranchPipelineResponse, FORCE_PUSH_PROMPT,
    NO_VERIFY_FORCE_PUSH_PROMPT, NO_VERIFY_PUSH_PROMPT, PULL_PROMPT, PUSH_PROMPT,
};
use super::pr_status::{collect_branch_refresh_results, BranchRefresh};
use super::pull::{
    build_pull_pipeline_steps, claim_or_queue_pull_for_branch, finish_immediate_pull_session,
    pull_or_queue_branch_with, PullDisposition,
};
use super::push::{
    build_push_pipeline_steps, insert_running_push_pipeline_session,
    queue_push_pipeline_if_branch_busy, queue_push_pipeline_locked, PushVariant,
};
use super::revert::{
    build_revert_pipeline_steps, insert_running_revert_pipeline_session_if_idle,
    REVERT_BRANCH_BUSY, REVERT_WORKTREE_BUSY,
};
use crate::git;
use crate::session_commands::BranchSessionLaunchStatus;
use crate::session_runner;
use crate::store::{
    self, FailureStrategy, PipelineExecution, PipelineKind, PipelineStep, StepStatus, Store,
};

mod commit;
mod create_pr;
mod deferred;
mod git_pipeline;
mod pr_status;
mod pull;
mod push;
mod revert;

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

fn pipeline_context(branch: &store::Branch) -> BranchPipelineContext {
    BranchPipelineContext {
        branch: branch.clone(),
        working_dir: PathBuf::from("/tmp/staged-test"),
        workspace_name: None,
        remote_working_dir: None,
    }
}
