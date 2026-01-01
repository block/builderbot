//! Diff module - core data types and operations for the diff viewer.
//!
//! This module provides:
//! - `types`: Core data structures (DiffId, FileDiff, etc.)
//! - `git`: Git operations for computing diffs
//! - `review`: SQLite-backed review storage

pub mod git;
pub mod review;
pub mod types;

// Re-export types used by lib.rs Tauri commands
pub use git::{
    compute_diff, get_refs, get_repo_info, last_commit_message, open_repo, resolve_ref, GitRef,
    RepoInfo, WORKDIR,
};
pub use review::{
    export_markdown, get_store, init_store, Comment, Edit, NewComment, NewEdit, Review,
};
pub use types::{DiffId, FileDiff};
