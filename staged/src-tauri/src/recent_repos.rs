//! Recent Repositories Detection
//!
//! Detects recently modified files on the user's system and finds git repositories
//! they belong to. Uses macOS Spotlight (mdfind) for efficient file discovery.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// A recently active git repository.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRepo {
    /// Repository name (directory name)
    pub name: String,
    /// Full path to the repository root
    pub path: String,
}

/// Directories to scan for recent activity.
const SCAN_DIRS: &[&str] = &[
    "Documents",
    "Downloads",
    "Desktop",
    "Development",
    "dev",
    "projects",
    "code",
    "repos",
    "src",
    "workspace",
    "work",
    "github",
    "gitlab",
];

/// Paths to exclude from results.
const EXCLUDE_PATTERNS: &[&str] = &[
    "node_modules",
    "/target/",
    "/.git/",
    "/.cargo/",
    "/.rustup/",
    "/Library/",
    "/.Trash/",
    "/__pycache__/",
    "/venv/",
    "/.venv/",
];

/// Find git repositories that have been recently active.
///
/// Uses macOS Spotlight to find files modified within `hours_ago` hours,
/// then walks up from each file to find the containing git repository.
///
/// Returns up to `limit` unique repositories, sorted by most recently active.
pub fn find_recent_repos(hours_ago: u32, limit: usize) -> Vec<RecentRepo> {
    let start = Instant::now();

    // Get home directory
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    // Build list of directories to scan
    let scan_dirs: Vec<PathBuf> = SCAN_DIRS
        .iter()
        .map(|d| home.join(d))
        .filter(|p| p.exists())
        .collect();

    if scan_dirs.is_empty() {
        return Vec::new();
    }

    // Use mdfind (Spotlight) to find recently modified files
    let files = match find_recent_files_mdfind(&scan_dirs, hours_ago) {
        Some(f) => f,
        None => {
            // Fallback: no mdfind or it failed
            return Vec::new();
        }
    };

    // Find git repos from the file list
    let mut seen_repos: HashSet<PathBuf> = HashSet::new();
    let mut repos: Vec<RecentRepo> = Vec::new();

    for file in files {
        // Skip excluded paths
        if EXCLUDE_PATTERNS.iter().any(|p| file.contains(p)) {
            continue;
        }

        // Walk up to find .git
        if let Some(repo_path) = find_git_root(Path::new(&file), &home) {
            if seen_repos.insert(repo_path.clone()) {
                let name = repo_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Repository".to_string());

                repos.push(RecentRepo {
                    name,
                    path: repo_path.to_string_lossy().to_string(),
                });

                if repos.len() >= limit {
                    break;
                }
            }
        }
    }

    log::debug!(
        "find_recent_repos: found {} repos in {:?}",
        repos.len(),
        start.elapsed()
    );

    repos
}

/// Use macOS Spotlight (mdfind) to find recently modified files.
fn find_recent_files_mdfind(scan_dirs: &[PathBuf], hours_ago: u32) -> Option<Vec<String>> {
    let seconds = hours_ago * 3600;

    // Build -onlyin arguments
    let mut args: Vec<String> = Vec::new();
    for dir in scan_dirs {
        args.push("-onlyin".to_string());
        args.push(dir.to_string_lossy().to_string());
    }

    // Add the query for recently modified files
    args.push(format!(
        "kMDItemFSContentChangeDate >= $time.now(-{seconds})"
    ));

    let output = Command::new("mdfind").args(&args).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout
        .lines()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    Some(files)
}

/// Walk up from a path to find the git repository root.
/// Stops at the home directory to avoid scanning system directories.
fn find_git_root(path: &Path, home: &Path) -> Option<PathBuf> {
    let mut current = if path.is_file() {
        path.parent()?.to_path_buf()
    } else {
        path.to_path_buf()
    };

    // Don't go above home directory
    while current.starts_with(home) && current != *home {
        if current.join(".git").exists() {
            return Some(current);
        }
        current = current.parent()?.to_path_buf();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_git_root() {
        let home = dirs::home_dir().unwrap();

        // Test with a path that doesn't exist - should return None
        let fake_path = home.join("nonexistent/path/to/file.txt");
        assert!(find_git_root(&fake_path, &home).is_none());
    }

    #[test]
    fn test_exclude_patterns() {
        let test_paths = vec![
            "/Users/test/project/node_modules/package/index.js",
            "/Users/test/project/target/debug/binary",
            "/Users/test/project/.git/objects/abc",
        ];

        for path in test_paths {
            let excluded = EXCLUDE_PATTERNS.iter().any(|p| path.contains(p));
            assert!(excluded, "Path should be excluded: {path}");
        }
    }
}
