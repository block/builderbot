//! Centralized data-directory helpers.
//!
//! All application data lives under `~/.staged/`.

use std::path::PathBuf;

/// Base data directory: `~/.staged/`
pub fn data_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".staged"))
}

/// Directory for bare/clone repos: `~/.staged/repos/`
pub fn repos_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("repos"))
}

/// Derive the local clone path for a GitHub repo: `<repos_dir>/<owner>/<repo>/`.
///
/// Returns `None` if the data directory can't be determined.
pub fn clone_path_for(github_repo: &str) -> Option<PathBuf> {
    repos_dir().map(|d| d.join(github_repo))
}

/// Root directory for workspace-scoped local data: `~/.staged/workspaces/`
pub fn workspaces_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("workspaces"))
}

/// Legacy directory for local worktrees: `~/.staged/worktrees/`
pub fn legacy_worktrees_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("worktrees"))
}

/// Directory for local git worktrees: `~/.staged/workspaces/local/`
pub fn worktrees_dir() -> Option<PathBuf> {
    workspaces_dir().map(|d| d.join("local"))
}

/// Path for the SQLite database: `~/.staged/data.db`
pub fn db_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("data.db"))
}

/// Migrate local worktrees from the legacy path to the new workspace-scoped path.
///
/// Moves entries from `~/.staged/worktrees/` to `~/.staged/workspaces/local/`.
/// Safe to call repeatedly.
pub fn migrate_legacy_worktrees_layout() {
    let Some(old_dir) = legacy_worktrees_dir() else {
        return;
    };
    let Some(new_dir) = worktrees_dir() else {
        return;
    };
    if old_dir == new_dir || !old_dir.exists() {
        return;
    }

    log::info!(
        "Migrating local worktrees from {} to {}",
        old_dir.display(),
        new_dir.display()
    );
    migrate_directory_contents(&old_dir, &new_dir);

    match std::fs::read_dir(&old_dir) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                if let Err(e) = std::fs::remove_dir(&old_dir) {
                    log::warn!(
                        "Failed to remove empty legacy worktrees dir {}: {e}",
                        old_dir.display()
                    );
                }
            }
        }
        Err(e) => {
            log::warn!(
                "Cannot verify legacy worktrees dir after migration {}: {e}",
                old_dir.display()
            );
        }
    }
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
