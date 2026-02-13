//! Centralized data-directory helpers.
//!
//! Uses `dirs::data_dir()` to follow platform conventions:
//! - Linux: `$XDG_DATA_HOME/staged/` (defaults to `~/.local/share/staged/`)
//! - macOS: `~/Library/Application Support/staged/`

use std::path::PathBuf;

/// Base data directory: `<data_dir>/staged/`
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("staged"))
}

/// Directory for bare/clone repos: `<data_dir>/staged/repos/`
pub fn repos_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("repos"))
}

/// Directory for worktrees: `<data_dir>/staged/worktrees/`
pub fn worktrees_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("worktrees"))
}

/// Path for the SQLite database: `<data_dir>/staged/data.db`
pub fn db_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("data.db"))
}
