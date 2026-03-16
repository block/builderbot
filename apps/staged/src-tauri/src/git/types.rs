// Re-export all diff types from the shared git-diff crate.
// Staged's git module uses these types throughout, so we re-export them
// here to keep existing imports working.
pub use git_diff::*;
