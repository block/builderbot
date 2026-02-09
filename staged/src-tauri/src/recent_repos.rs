//! Recent Repositories Detection
//!
//! Finds recently active git repositories using macOS Spotlight (mdfind).
//! Scans common dev directories for files modified within a time window,
//! then walks up to find containing git repos.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A recently active git repository.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRepo {
    pub name: String,
    pub path: String,
}

/// Directories under $HOME to scan for recent activity.
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
/// Returns up to `limit` unique repositories.
pub fn find_recent_repos(hours_ago: u32, limit: usize) -> Vec<RecentRepo> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let scan_dirs: Vec<PathBuf> = SCAN_DIRS
        .iter()
        .map(|d| home.join(d))
        .filter(|p| p.exists())
        .collect();

    if scan_dirs.is_empty() {
        return Vec::new();
    }

    let files = match find_recent_files_mdfind(&scan_dirs, hours_ago) {
        Some(f) => f,
        None => return Vec::new(),
    };

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut repos = Vec::new();

    for file in files {
        if EXCLUDE_PATTERNS.iter().any(|p| file.contains(p)) {
            continue;
        }

        if let Some(repo_path) = find_git_root(Path::new(&file), &home) {
            if seen.insert(repo_path.clone()) {
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

    repos
}

/// Use macOS Spotlight (mdfind) to find recently modified files.
fn find_recent_files_mdfind(scan_dirs: &[PathBuf], hours_ago: u32) -> Option<Vec<String>> {
    let seconds = hours_ago * 3600;

    let mut args: Vec<String> = Vec::new();
    for dir in scan_dirs {
        args.push("-onlyin".to_string());
        args.push(dir.to_string_lossy().to_string());
    }
    args.push(format!(
        "kMDItemFSContentChangeDate >= $time.now(-{seconds})"
    ));

    let output = Command::new("mdfind").args(&args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(
        stdout
            .lines()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
    )
}

/// Walk up from a path to find the git repository root.
/// Stops at the home directory boundary.
fn find_git_root(path: &Path, home: &Path) -> Option<PathBuf> {
    let mut current = if path.is_file() {
        path.parent()?.to_path_buf()
    } else {
        path.to_path_buf()
    };

    while current.starts_with(home) && current != *home {
        if current.join(".git").exists() {
            return Some(current);
        }
        current = current.parent()?.to_path_buf();
    }

    None
}
