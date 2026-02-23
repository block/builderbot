//! Git diff computation library.
//!
//! Provides file listing, content diffing, and line alignment mapping
//! for rendering side-by-side diffs. Designed to be reusable across
//! multiple frontend applications.

mod cli;
mod diff;
mod files;
mod refs;
mod types;

pub use cli::GitError;
pub use diff::{get_file_diff, get_unified_diff, list_diff_files};
pub use files::{get_file_at_ref, search_files};
pub use refs::{detect_default_branch, merge_base};
pub use types::*;
