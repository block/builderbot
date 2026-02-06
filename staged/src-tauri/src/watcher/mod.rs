//! File system watcher for detecting repository changes.
//!
//! Architecture: A dedicated background thread manages multiple watchers (one per repo).
//! Commands are sent via channel, so the main thread never blocks on watcher
//! setup/teardown. Events include a watch ID so the frontend can identify which
//! repo changed. Watchers are only dropped when explicitly unwatched (e.g., when
//! closing a tab with no other tabs using that repo).

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, RecommendedCache};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Event name for file change notifications sent to frontend.
const EVENT_FILES_CHANGED: &str = "files-changed";

/// Payload sent with files-changed events
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FilesChangedPayload {
    watch_id: u64,
}

/// Commands sent to the watcher background thread
enum WatcherCommand {
    /// Start watching a repository (idempotent - no-op if already watching)
    Watch { path: PathBuf, watch_id: u64 },
    /// Stop watching a repository
    Unwatch { path: PathBuf },
}

/// Active watcher entry
struct WatcherEntry {
    /// Shared watch ID that can be updated if frontend re-registers with a new ID
    watch_id: Arc<AtomicU64>,
    #[allow(dead_code)] // Dropping this stops the watcher
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

/// Handle to the watcher background thread.
/// Clone-able and thread-safe - just wraps a channel sender.
pub struct WatcherHandle {
    tx: Sender<WatcherCommand>,
}

impl WatcherHandle {
    /// Spawn the watcher background thread and return a handle to it.
    pub fn new(app_handle: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let mut watchers: HashMap<PathBuf, WatcherEntry> = HashMap::new();

            for cmd in rx {
                match cmd {
                    WatcherCommand::Watch { path, watch_id } => {
                        // If already watching, just update the watch_id
                        if let Some(entry) = watchers.get(&path) {
                            let old_id = entry.watch_id.swap(watch_id, Ordering::SeqCst);
                            log::info!(
                                "Updated watch_id for {} from {} to {}",
                                path.display(),
                                old_id,
                                watch_id
                            );
                            continue;
                        }

                        // Setup new watcher with shared atomic watch_id
                        let watch_id_arc = Arc::new(AtomicU64::new(watch_id));
                        match create_watcher(&path, Arc::clone(&watch_id_arc), &app_handle) {
                            Ok(debouncer) => {
                                watchers.insert(
                                    path.clone(),
                                    WatcherEntry {
                                        watch_id: watch_id_arc,
                                        debouncer,
                                    },
                                );
                                log::info!(
                                    "Started watching {} (watch_id {}), total watchers: {}",
                                    path.display(),
                                    watch_id,
                                    watchers.len()
                                );
                            }
                            Err(e) => {
                                log::error!(
                                    "Failed to create watcher for {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }
                    WatcherCommand::Unwatch { path } => {
                        if watchers.remove(&path).is_some() {
                            log::info!(
                                "Stopped watching {}, total watchers: {}",
                                path.display(),
                                watchers.len()
                            );
                        }
                    }
                }
            }
        });

        Self { tx }
    }

    /// Start watching a repository (idempotent).
    /// Returns immediately - actual setup happens on background thread.
    pub fn watch(&self, path: PathBuf, watch_id: u64) {
        let _ = self.tx.send(WatcherCommand::Watch { path, watch_id });
    }

    /// Stop watching a repository.
    /// Returns immediately - actual teardown happens on background thread.
    pub fn unwatch(&self, path: PathBuf) {
        let _ = self.tx.send(WatcherCommand::Unwatch { path });
    }
}

/// Create a new debounced watcher for the given repository.
fn create_watcher(
    repo_path: &Path,
    watch_id: Arc<AtomicU64>,
    app_handle: &AppHandle,
) -> Result<Debouncer<RecommendedWatcher, RecommendedCache>, String> {
    let gitignore = build_gitignore(repo_path);
    let repo_path_for_filter = repo_path.to_path_buf();
    let repo_path_for_log = repo_path.to_path_buf();
    let app_handle = app_handle.clone();

    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        None,
        move |result: Result<Vec<DebouncedEvent>, Vec<notify::Error>>| match result {
            Ok(events) => {
                let relevant_paths: Vec<_> = events
                    .iter()
                    .flat_map(|e| e.paths.iter())
                    .filter(|p| should_trigger_refresh(p, &repo_path_for_filter, &gitignore))
                    .collect();

                if !relevant_paths.is_empty() {
                    let current_watch_id = watch_id.load(Ordering::SeqCst);
                    log::debug!(
                        "Emitting files-changed for {} (watch_id {}), {} relevant paths",
                        repo_path_for_log.display(),
                        current_watch_id,
                        relevant_paths.len()
                    );
                    let _ = app_handle.emit(
                        EVENT_FILES_CHANGED,
                        FilesChangedPayload {
                            watch_id: current_watch_id,
                        },
                    );
                }
            }
            Err(errors) => {
                for e in errors {
                    log::warn!("Watcher error: {e}");
                }
            }
        },
    )
    .map_err(|e| e.to_string())?;

    debouncer
        .watch(repo_path, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;

    Ok(debouncer)
}

/// Build a Gitignore matcher for the repository.
fn build_gitignore(repo_path: &Path) -> Arc<Gitignore> {
    let mut builder = GitignoreBuilder::new(repo_path);

    // Add .gitignore in repo root
    let gitignore_path = repo_path.join(".gitignore");
    if gitignore_path.exists() {
        let _ = builder.add(&gitignore_path);
    }

    // Add .git/info/exclude
    let exclude_path = repo_path.join(".git/info/exclude");
    if exclude_path.exists() {
        let _ = builder.add(&exclude_path);
    }

    // Add global gitignore
    if let Some(global_path) = find_global_gitignore() {
        let _ = builder.add(&global_path);
    }

    Arc::new(
        builder
            .build()
            .unwrap_or_else(|_| GitignoreBuilder::new(repo_path).build().unwrap()),
    )
}

/// Find the global gitignore file location.
fn find_global_gitignore() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GIT_CONFIG_GLOBAL") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(config_home).join("git/ignore");
        if path.exists() {
            return Some(path);
        }
    }

    if let Some(home) = dirs::home_dir() {
        let path = home.join(".config/git/ignore");
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Determine if a file change should trigger a status refresh.
fn should_trigger_refresh(path: &Path, repo_root: &Path, gitignore: &Gitignore) -> bool {
    let relative = match path.strip_prefix(repo_root) {
        Ok(rel) => rel,
        Err(_) => return false,
    };

    let path_str = relative.to_string_lossy();

    // .git/ directory: only trigger on key state files
    if path_str.starts_with(".git/") || path_str == ".git" {
        if path_str == ".git/index" || path_str == ".git/HEAD" || path_str.starts_with(".git/refs/")
        {
            return true;
        }
        return false;
    }

    // Working tree: use gitignore rules
    let is_dir = path.is_dir();
    if gitignore
        .matched_path_or_any_parents(relative, is_dir)
        .is_ignore()
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_gitignore(repo: &Path) -> Arc<Gitignore> {
        Arc::new(GitignoreBuilder::new(repo).build().unwrap())
    }

    fn gitignore_with_patterns(repo: &Path, patterns: &[&str]) -> Arc<Gitignore> {
        let mut builder = GitignoreBuilder::new(repo);
        for pattern in patterns {
            builder.add_line(None, pattern).unwrap();
        }
        Arc::new(builder.build().unwrap())
    }

    #[test]
    fn test_git_directory_filtering() {
        let repo = Path::new("/repo");
        let gi = empty_gitignore(repo);

        assert!(should_trigger_refresh(
            Path::new("/repo/.git/index"),
            repo,
            &gi
        ));
        assert!(should_trigger_refresh(
            Path::new("/repo/.git/HEAD"),
            repo,
            &gi
        ));
        assert!(should_trigger_refresh(
            Path::new("/repo/.git/refs/heads/main"),
            repo,
            &gi
        ));

        assert!(!should_trigger_refresh(
            Path::new("/repo/.git/objects/ab/cdef123"),
            repo,
            &gi
        ));
        assert!(!should_trigger_refresh(
            Path::new("/repo/.git/logs/HEAD"),
            repo,
            &gi
        ));
    }

    #[test]
    fn test_gitignore_filtering() {
        let repo = Path::new("/repo");
        let gi = gitignore_with_patterns(repo, &["node_modules/", "*.pyc", "build/"]);

        assert!(should_trigger_refresh(
            Path::new("/repo/src/main.rs"),
            repo,
            &gi
        ));
        assert!(!should_trigger_refresh(
            Path::new("/repo/node_modules/foo/bar.js"),
            repo,
            &gi
        ));
        assert!(!should_trigger_refresh(
            Path::new("/repo/foo.pyc"),
            repo,
            &gi
        ));
    }
}
