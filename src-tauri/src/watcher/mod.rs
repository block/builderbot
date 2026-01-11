//! File system watcher for detecting repository changes.
//!
//! Simplified watching strategy (Phase 2 of TSK-894):
//! - Single recursive watch on repo root
//! - Filter events using `.gitignore` rules (via `ignore` crate)
//! - Special handling for `.git/` directory (only key state files trigger refresh)
//!
//! This replaces the expensive walk-entire-repo approach. Event filtering
//! happens at notification time rather than watch setup time.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, RecommendedCache};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Callback type for when the watcher detects changes
pub type OnChangeCallback = Box<dyn Fn() + Send + 'static>;

/// Trait for file system watching implementations.
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
///
/// Uses a single recursive watch on the repo root. Events are filtered using:
/// 1. `.gitignore` rules for working tree files
/// 2. Hardcoded rules for `.git/` internals (only index, HEAD, refs trigger)
pub struct NotifyWatcher {
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
    repo_path: Option<PathBuf>,
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
            repo_path: None,
        }
    }
}

impl WatcherManager for NotifyWatcher {
    fn start(&mut self, repo_path: &Path, on_change: OnChangeCallback) -> Result<(), WatcherError> {
        // Stop any existing watcher
        self.stop();

        // Build gitignore matcher for this repo
        let gitignore = build_gitignore(repo_path);
        let repo_path_for_filter = repo_path.to_path_buf();

        // Debouncer timing:
        // - 500ms quiet period before firing
        // - Coalesces rapid changes (e.g., git operations touching many files)
        let mut debouncer = new_debouncer(
            Duration::from_millis(500),
            None, // Default tick_rate (timeout / 4 = 125ms)
            move |result: Result<Vec<DebouncedEvent>, Vec<notify::Error>>| {
                match result {
                    Ok(events) => {
                        // Filter to relevant events
                        let relevant_paths: Vec<_> = events
                            .iter()
                            .flat_map(|e| e.paths.iter())
                            .filter(|p| {
                                should_trigger_refresh(p, &repo_path_for_filter, &gitignore)
                            })
                            .collect();

                        if !relevant_paths.is_empty() {
                            log::debug!(
                                "Watcher detected {} relevant changes",
                                relevant_paths.len()
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

        // Watch repo root recursively
        // FSEvents on macOS is efficient with recursive watches
        debouncer.watch(repo_path, RecursiveMode::Recursive)?;

        self.debouncer = Some(debouncer);
        self.repo_path = Some(repo_path.to_path_buf());

        log::info!(
            "Started watching repository (recursive): {}",
            repo_path.display()
        );
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut debouncer) = self.debouncer.take() {
            if let Some(ref path) = self.repo_path {
                let _ = debouncer.unwatch(path);
            }
            log::info!("Stopped watching repository");
        }
        self.repo_path = None;
    }
}

/// Build a Gitignore matcher for the repository.
/// Loads .gitignore, .git/info/exclude, and global gitignore.
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

    // Add global gitignore (e.g., ~/.config/git/ignore)
    if let Some(global_path) = find_global_gitignore() {
        let _ = builder.add(&global_path);
    }

    Arc::new(builder.build().unwrap_or_else(|_| {
        // Fallback to empty gitignore if building fails
        GitignoreBuilder::new(repo_path).build().unwrap()
    }))
}

/// Find the global gitignore file location.
fn find_global_gitignore() -> Option<PathBuf> {
    // Check GIT_CONFIG_GLOBAL or standard locations
    if let Ok(path) = std::env::var("GIT_CONFIG_GLOBAL") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    // Standard XDG location
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(config_home).join("git/ignore");
        if path.exists() {
            return Some(path);
        }
    }

    // Fallback to ~/.config/git/ignore
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

    // === .git/ directory handling ===
    // Only trigger on files that indicate actual state changes
    if path_str.starts_with(".git/") || path_str == ".git" {
        // Key files that indicate state changes
        if path_str == ".git/index" || path_str == ".git/HEAD" || path_str.starts_with(".git/refs/")
        {
            return true;
        }
        // Ignore everything else in .git/
        return false;
    }

    // === Working tree: use gitignore rules ===
    // Use matched_path_or_any_parents to handle files inside ignored directories
    // e.g., "node_modules/" pattern should match "node_modules/foo/bar.js"
    let is_dir = path.is_dir();
    if gitignore
        .matched_path_or_any_parents(relative, is_dir)
        .is_ignore()
    {
        return false;
    }

    // Not ignored - trigger refresh
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

        // Should trigger - key git state files
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

        // Should NOT trigger - git internals
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
        assert!(!should_trigger_refresh(
            Path::new("/repo/.git/hooks/pre-commit"),
            repo,
            &gi
        ));
    }

    #[test]
    fn test_gitignore_filtering() {
        let repo = Path::new("/repo");
        let gi = gitignore_with_patterns(repo, &["node_modules/", "*.pyc", "build/"]);

        // Should trigger - not ignored
        assert!(should_trigger_refresh(
            Path::new("/repo/src/main.rs"),
            repo,
            &gi
        ));
        assert!(should_trigger_refresh(
            Path::new("/repo/README.md"),
            repo,
            &gi
        ));

        // Should NOT trigger - matches gitignore patterns
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
        assert!(!should_trigger_refresh(
            Path::new("/repo/build/output.js"),
            repo,
            &gi
        ));
    }

    #[test]
    fn test_nested_ignored_directories() {
        let repo = Path::new("/repo");
        let gi = gitignore_with_patterns(repo, &["node_modules/", "target/"]);

        // Nested ignored directories
        assert!(!should_trigger_refresh(
            Path::new("/repo/packages/foo/node_modules/bar/index.js"),
            repo,
            &gi
        ));
        assert!(!should_trigger_refresh(
            Path::new("/repo/crates/core/target/debug/libcore.rlib"),
            repo,
            &gi
        ));
    }
}
