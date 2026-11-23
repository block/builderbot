//! Centralized data-directory helpers.
//!
//! All application data lives under `~/.staged/`, unless [`DATA_DIR_ENV`]
//! names another directory. The paths documented below as `~/.staged/…` all
//! hang off [`data_dir`], so they move with it.

use std::ffi::OsString;
use std::path::PathBuf;

/// Environment variable that moves the whole data directory, so a development
/// build can run beside an installed Staged without opening (and migrating)
/// its database. Only an absolute path is honoured; see [`resolve_data_dir`].
pub const DATA_DIR_ENV: &str = "STAGED_DATA_DIR";

/// Base data directory: `$STAGED_DATA_DIR` when it is set to an absolute
/// path, else `~/.staged/`.
pub fn data_dir() -> Option<PathBuf> {
    let requested = std::env::var_os(DATA_DIR_ENV);
    if let Some(ignored) = ignored_data_dir_override(requested.as_ref()) {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            log::warn!(
                "Ignoring ${DATA_DIR_ENV}={ignored:?}: it must be an absolute path. \
                 Using the default data directory."
            );
        });
    }
    resolve_data_dir(requested, dirs::home_dir())
}

/// [`data_dir`] over its two inputs: the override, if usable, else `.staged`
/// under `home`.
///
/// A relative override is ignored, not resolved: the working directory is not
/// one place (the app is started from the Finder, a terminal and `cargo`),
/// and every caller reads this again, so one that changed directory would
/// see a different data directory than the rest. An empty value is relative.
fn resolve_data_dir(requested: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    match requested.map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Some(dir),
        _ => home.map(|d| d.join(".staged")),
    }
}

/// The override [`resolve_data_dir`] will not use, for the warning.
fn ignored_data_dir_override(requested: Option<&OsString>) -> Option<&OsString> {
    requested.filter(|value| !std::path::Path::new(value).is_absolute())
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

/// Root for app-managed runtime packages (the managed Node.js runtime and,
/// eventually, npm-installed ACP bridges): `~/.staged/packages/`
///
/// Shared by every running Staged instance; mutations must hold the
/// cross-process lock (see `managed_node`).
pub fn packages_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("packages"))
}

/// Project transfer artifacts: `~/.staged/transfers/<transfer id>/`, holding
/// each repository's bundle as the backup of what was transferred.
pub fn transfers_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("transfers"))
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

/// Per-project data directory, scoped by location:
/// `~/.staged/workspaces/<local|remote>/projects/<project_id>/`
pub fn project_data_dir(
    location: crate::store::ProjectLocation,
    project_id: &str,
) -> Option<PathBuf> {
    workspaces_dir().map(|d| d.join(location.as_str()).join("projects").join(project_id))
}

/// Path for the SQLite database: `~/.staged/data.db`
pub fn db_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("data.db"))
}

/// Migrate local worktrees from the legacy path to the new workspace-scoped path.
///
/// Moves entries from `~/.staged/worktrees/` to `~/.staged/workspaces/local/`.
/// Safe to call repeatedly.
pub fn migrate_legacy_worktrees_layout() -> Result<(), String> {
    let Some(old_dir) = legacy_worktrees_dir() else {
        return Ok(());
    };
    let Some(new_dir) = worktrees_dir() else {
        return Ok(());
    };
    if old_dir == new_dir || !old_dir.exists() {
        return Ok(());
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
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Option<PathBuf> {
        Some(PathBuf::from("/Users/someone"))
    }

    #[test]
    fn data_dir_defaults_to_dot_staged_under_home() {
        assert_eq!(
            resolve_data_dir(None, home()),
            Some(PathBuf::from("/Users/someone/.staged"))
        );
        assert_eq!(resolve_data_dir(None, None), None);
    }

    #[test]
    fn data_dir_uses_an_absolute_override() {
        let requested = Some(OsString::from("/tmp/staged-isolated/.staged"));
        assert_eq!(ignored_data_dir_override(requested.as_ref()), None);
        assert_eq!(
            resolve_data_dir(requested.clone(), home()),
            Some(PathBuf::from("/tmp/staged-isolated/.staged"))
        );
        // The override does not need a home directory to fall back on.
        assert_eq!(
            resolve_data_dir(requested, None),
            Some(PathBuf::from("/tmp/staged-isolated/.staged"))
        );
    }

    #[test]
    fn data_dir_ignores_a_relative_override() {
        for relative in ["staged-data", "./staged-data", "../staged-data", ""] {
            let requested = Some(OsString::from(relative));
            assert_eq!(
                ignored_data_dir_override(requested.as_ref()),
                requested.as_ref(),
                "{relative:?}"
            );
            assert_eq!(
                resolve_data_dir(requested, home()),
                Some(PathBuf::from("/Users/someone/.staged")),
                "{relative:?}"
            );
        }
    }
}
