//! Diff module - core data types and operations for the diff viewer.
//!
//! This module provides:
//! - `types`: Core data structures (DiffId, FileDiff, etc.)
//! - `git`: Git operations for computing diffs
//! - `review`: SQLite-backed review storage
//! - `actions`: Git actions (commit, discard, etc.)
//! - `watcher`: File system watcher for detecting changes

pub mod actions;
pub mod git;
pub mod review;
pub mod types;
pub mod watcher;

// Re-export commonly used types
pub use actions::{amend, commit, discard_file, discard_region, stage_file, unstage_file};
pub use git::{
    compute_diff, current_branch, get_refs, get_repo_info, last_commit_message, open_repo,
    resolve_ref, GitError, GitRef, RefType, RepoInfo,
};
pub use review::{
    export_markdown, get_store, init_store, Comment, Edit, NewComment, NewEdit, Review,
    ReviewError, ReviewStore, Selection,
};
pub use types::{Alignment, ChangeKind, DiffId, File, FileContent, FileDiff, Span};
pub use watcher::RepoWatcher;
