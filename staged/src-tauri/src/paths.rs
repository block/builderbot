//! Centralized data-directory helpers.
//!
//! All application data lives under `~/.staged/`.

use std::path::PathBuf;

/// Base data directory: `~/.staged/`
pub fn data_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".staged"))
}

/// Legacy data directory used before the move to `~/.staged/`.
/// On macOS this was `~/Library/Application Support/staged/`.
pub fn legacy_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("staged"))
}

/// Directory for bare/clone repos: `~/.staged/repos/`
pub fn repos_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("repos"))
}

/// Directory for worktrees: `~/.staged/worktrees/`
pub fn worktrees_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("worktrees"))
}

/// Path for the SQLite database: `~/.staged/data.db`
pub fn db_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("data.db"))
}

/// Move all entries from `src` into `dst`, creating `dst` if needed.
/// Skips entries that already exist in `dst`. Logs warnings on failure.
pub fn migrate_directory_contents(src: &std::path::Path, dst: &std::path::Path) {
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Cannot read legacy dir {}: {e}", src.display());
            return;
        }
    };

    if let Err(e) = std::fs::create_dir_all(dst) {
        log::warn!("Cannot create target dir {}: {e}", dst.display());
        return;
    }

    for entry in entries.flatten() {
        let dest = dst.join(entry.file_name());
        if dest.exists() {
            log::info!("Skipping {} (already exists)", dest.display());
            continue;
        }
        if let Err(e) = std::fs::rename(entry.path(), &dest) {
            log::warn!(
                "Failed to migrate {} -> {}: {e}",
                entry.path().display(),
                dest.display()
            );
        }
    }
}
