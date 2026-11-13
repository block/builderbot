//! Shared project command implementations: one body per operation, used by
//! the Tauri commands, the web/control `dispatch` arms and the MCP tools.

mod add_repo;
mod create;
mod rename;
// The resolver has no caller yet: the control `dispatch` arms for
// `create_project` and `add_project_repo` run it once those commands are on
// the control allowlist. Only its monorepo rule is in use, by the MCP tool.
#[allow(dead_code)]
mod repo_spec;
mod setup;

pub(crate) use add_repo::{add_project_repo_and_setup, add_project_repo_impl};
pub(crate) use create::{create_project_and_setup, CreateProjectRequest};
pub(crate) use rename::rename_project_impl;
pub(crate) use repo_spec::is_monorepo;
