//! Shared project command implementations: one body per operation, used by
//! the Tauri commands, the web/control `dispatch` arms and the MCP tools.

mod add_repo;
mod branch_in_use;
mod branch_name;
mod control;
mod create;
mod creation_limit;
mod pr_note;
mod rename;
mod repo_spec;
mod setup;

pub(crate) use add_repo::{
    add_project_repo_and_setup, add_project_repo_impl, AddProjectRepoRequest,
};
pub(crate) use control::{
    control_add_repo_request, control_create_project_request, ControlAddRepo, ControlCreateProject,
};
pub(crate) use create::{create_project_and_setup, CreateProjectRequest, ProjectCreator};
pub(crate) use rename::rename_project_impl;
pub(crate) use repo_spec::{is_monorepo, RepoSpecRequest};
