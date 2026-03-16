// File browsing operations are now provided by the shared git-diff crate.
// Re-export the public API so existing imports in Staged continue to work.
pub use git_diff::{get_file_at_ref, search_files};
