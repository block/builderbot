// Diff computation is now provided by the shared git-diff crate.
// Re-export the public API so existing imports in Staged continue to work.
pub use git_diff::{get_file_diff, get_unified_diff, list_diff_files};
