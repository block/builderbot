//! File system watcher for detecting repository changes.
//!
//! This module provides the `WatcherManager` trait and implementations
//! for triggering status refreshes when files change.
//!
//! The current implementation uses `notify` with FSEvents on macOS.
//! Uses the `ignore` crate to respect .gitignore and skip ignored directories.

use ignore::WalkBuilder;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, RecommendedCache};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Callback type for when the watcher detects changes
pub type OnChangeCallback = Box<dyn Fn() + Send + 'static>;

/// Trait for file system watching implementations.
/// Easy to swap out for different strategies (polling, hooks, etc.)
pub trait WatcherManager: Send {
    /// Start watching a repository for changes.
    /// Calls `on_change` when relevant files change (debounced).
    fn start(&mut self, repo_path: &Path, on_change: OnChangeCallback) -> Result<(), WatcherError>;

    /// Stop watching the current repository.
    fn stop(&mut self);
}

#[derive(Debug)]
pub struct WatcherError {
    pub message: String,
}

impl std::fmt::Display for WatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WatcherError {}

impl From<notify::Error> for WatcherError {
    fn from(e: notify::Error) -> Self {
        WatcherError {
            message: e.to_string(),
        }
    }
}

/// FSEvents-based watcher using the `notify` crate.
/// Debounces rapid changes and filters irrelevant paths.
/// Uses `ignore` crate to respect .gitignore when setting up watches.
pub struct NotifyWatcher {
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
    watched_paths: HashSet<PathBuf>,
}

impl Default for NotifyWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl NotifyWatcher {
    pub fn new() -> Self {
        Self {
            debouncer: None,
            watched_paths: HashSet::new(),
        }
    }
}

impl WatcherManager for NotifyWatcher {
    fn start(&mut self, repo_path: &Path, on_change: OnChangeCallback) -> Result<(), WatcherError> {
        // Stop any existing watcher
        self.stop();

        let repo_path_for_filter = repo_path.to_path_buf();

        // Create debouncer with 1s timeout, no max_wait
        // - timeout: fire after 1s of quiet (coalesces rapid changes)
        // - no max_wait: let the backend throttle handle continuous activity
        let mut debouncer = new_debouncer(
            Duration::from_millis(1000),
            None,
            move |result: Result<Vec<DebouncedEvent>, Vec<notify::Error>>| {
                match result {
                    Ok(events) => {
                        // Check if any event is relevant (not filtered out)
                        let dominated_paths: Vec<_> =
                            events.iter().flat_map(|e| e.paths.iter()).collect();
                        let dominated_paths: Vec<_> = dominated_paths
                            .iter()
                            .filter(|p| should_trigger_refresh(p, &repo_path_for_filter))
                            .collect();

                        if !dominated_paths.is_empty() {
                            log::debug!(
                                "Watcher detected {} relevant changes",
                                dominated_paths.len()
                            );
                            on_change();
                        }
                    }
                    Err(errors) => {
                        for e in errors {
                            log::warn!("Watcher error: {}", e);
                        }
                    }
                }
            },
        )?;

        // Use ignore crate to walk only non-ignored directories
        // This respects .gitignore and skips node_modules, target/, etc.
        let mut dirs_to_watch: HashSet<PathBuf> = HashSet::new();

        // Always include repo root
        dirs_to_watch.insert(repo_path.to_path_buf());

        // Walk the repo, collecting directories that aren't ignored
        let walker = WalkBuilder::new(repo_path)
            .hidden(false) // Don't skip hidden files (we want .gitignore'd stuff skipped, not hidden)
            .git_ignore(true) // Respect .gitignore
            .git_global(true) // Respect global gitignore
            .git_exclude(true) // Respect .git/info/exclude
            .ignore(true) // Respect .ignore files
            .parents(true) // Check parent directories for ignore files
            .build();

        for entry in walker.flatten() {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                dirs_to_watch.insert(entry.path().to_path_buf());
            }
        }

        // Watch each directory non-recursively
        // (we've already enumerated the non-ignored dirs)
        for dir in &dirs_to_watch {
            if let Err(e) = debouncer.watch(dir, RecursiveMode::NonRecursive) {
                log::warn!("Failed to watch {}: {}", dir.display(), e);
            }
        }

        // Also watch .git directory for index/HEAD changes
        let git_dir = repo_path.join(".git");
        if git_dir.exists() {
            // Watch .git recursively since it's not walked by ignore crate
            debouncer.watch(&git_dir, RecursiveMode::Recursive)?;
            dirs_to_watch.insert(git_dir);
        }

        self.debouncer = Some(debouncer);
        self.watched_paths = dirs_to_watch;

        log::info!("Started watching repository: {}", repo_path.display());
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut debouncer) = self.debouncer.take() {
            for path in &self.watched_paths {
                let _ = debouncer.unwatch(path);
            }
            log::info!("Stopped watching repository");
        }
        self.watched_paths.clear();
    }
}

/// Determine if a file change should trigger a status refresh.
/// Filters out noise like .git/objects, node_modules, etc.
fn should_trigger_refresh(path: &Path, repo_root: &Path) -> bool {
    let relative = match path.strip_prefix(repo_root) {
        Ok(rel) => rel,
        Err(_) => return false,
    };

    let path_str = relative.to_string_lossy();

    // Always trigger on key .git files
    if path_str == ".git/index" || path_str == ".git/HEAD" || path_str.starts_with(".git/refs/") {
        return true;
    }

    // Ignore internal git files that change frequently but don't affect status
    if path_str.starts_with(".git/objects/")
        || path_str.starts_with(".git/logs/")
        || path_str.starts_with(".git/hooks/")
        || path_str.starts_with(".git/info/")
        || path_str.contains(".git/fsmonitor")
        || path_str.ends_with(".lock")
    {
        return false;
    }

    // Ignore other .git internals we haven't explicitly allowed
    if path_str.starts_with(".git/") {
        return false;
    }

    // Ignore common build/dependency directories
    if path_str.starts_with("node_modules/")
        || path_str.starts_with("target/")
        || path_str.starts_with(".build/")
        || path_str.starts_with("build/")
        || path_str.starts_with("dist/")
        || path_str.starts_with(".next/")
        || path_str.starts_with("__pycache__/")
        || path_str.starts_with(".pytest_cache/")
        || path_str.starts_with("venv/")
        || path_str.starts_with(".venv/")
    {
        return false;
    }

    // Ignore common temporary/generated files
    if path_str.ends_with(".pyc")
        || path_str.ends_with(".pyo")
        || path_str.ends_with(".class")
        || path_str.ends_with(".o")
        || path_str.ends_with(".a")
        || path_str.ends_with(".so")
        || path_str.ends_with(".dylib")
        || path_str.ends_with("~")
        || path_str.ends_with(".swp")
        || path_str.ends_with(".swo")
        || path_str.contains(".DS_Store")
    {
        return false;
    }

    // Everything else triggers a refresh
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_should_trigger_refresh() {
        let repo = Path::new("/repo");

        // Should trigger
        assert!(should_trigger_refresh(Path::new("/repo/src/main.rs"), repo));
        assert!(should_trigger_refresh(Path::new("/repo/.git/index"), repo));
        assert!(should_trigger_refresh(Path::new("/repo/.git/HEAD"), repo));
        assert!(should_trigger_refresh(
            Path::new("/repo/.git/refs/heads/main"),
            repo
        ));
        assert!(should_trigger_refresh(Path::new("/repo/README.md"), repo));

        // Should NOT trigger
        assert!(!should_trigger_refresh(
            Path::new("/repo/.git/objects/ab/cdef123"),
            repo
        ));
        assert!(!should_trigger_refresh(
            Path::new("/repo/.git/logs/HEAD"),
            repo
        ));
        assert!(!should_trigger_refresh(
            Path::new("/repo/node_modules/foo/bar.js"),
            repo
        ));
        assert!(!should_trigger_refresh(
            Path::new("/repo/target/debug/build"),
            repo
        ));
        assert!(!should_trigger_refresh(
            Path::new("/repo/.git/index.lock"),
            repo
        ));
        assert!(!should_trigger_refresh(Path::new("/repo/foo.pyc"), repo));
    }
}
