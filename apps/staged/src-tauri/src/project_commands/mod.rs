//! Shared project command implementations: one body per operation, used by
//! the Tauri commands, the web/control `dispatch` arms and the MCP tools.

mod add_repo;
mod create;
mod rename;
mod setup;

pub(crate) use add_repo::{add_project_repo_and_setup, add_project_repo_impl};
pub(crate) use create::{create_project_and_setup, CreateProjectRequest};
pub(crate) use rename::rename_project_impl;
