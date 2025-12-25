//! Git operations for Staged
//!
//! Pure git2 operations with no Tauri dependency.
//! All functions are stateless - they discover the repo fresh each call.

mod commit;
pub mod diff;
pub mod provider;
mod repo;
mod staging;
mod status;

use serde::{Deserialize, Serialize};

// Re-export public types (used by Tauri commands)
pub use commit::CommitResult;
pub use diff::FileDiff;
pub use provider::AdaptiveProvider;
pub use status::GitStatus;

// Re-export public functions (used by Tauri commands)
pub use commit::{amend_commit, create_commit, get_last_commit_message};
pub use diff::{get_file_diff, get_untracked_file_diff};
pub use staging::{discard_file, stage_all, stage_file, unstage_all, unstage_file};
pub use status::get_status;

/// Common error type for git operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitError {
    pub message: String,
}

impl From<git2::Error> for GitError {
    fn from(err: git2::Error) -> Self {
        GitError {
            message: err.message().to_string(),
        }
    }
}
