//! Differ — standalone diff viewer.
//!
//! A focused diff viewer that opens a git repository and shows diffs
//! using the shared git-diff crate and @builderbot/diff-viewer package.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use tauri::Manager;

// =============================================================================
// App state
// =============================================================================

struct AppState {
    repo_path: PathBuf,
}

// =============================================================================
// Types
// =============================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoInfo {
    path: String,
    branch: String,
    default_branch: String,
    commits_ahead: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitInfo {
    sha: String,
    short_sha: String,
    message: String,
    author: String,
    timestamp: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffFilesResponse {
    files: Vec<git_diff::FileDiffSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchArgs {
    repo_path: String,
    mode: Option<String>,
    commit: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    is_repo: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecentRepo {
    name: String,
    path: String,
}

// =============================================================================
// Commands: Git info
// =============================================================================

#[tauri::command(rename_all = "camelCase")]
fn get_repo_info(state: tauri::State<'_, Mutex<AppState>>) -> Result<RepoInfo, String> {
    let state = state.lock().unwrap();
    let repo = &state.repo_path;

    let branch = run_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "HEAD".to_string());

    let default_branch =
        git_diff::detect_default_branch(repo).unwrap_or_else(|_| "origin/main".to_string());

    let commits_ahead = run_git(
        repo,
        &["rev-list", "--count", &format!("{default_branch}..HEAD")],
    )
    .ok()
    .and_then(|s| s.trim().parse::<u32>().ok())
    .unwrap_or(0);

    Ok(RepoInfo {
        path: repo.display().to_string(),
        branch: branch.trim().to_string(),
        default_branch,
        commits_ahead,
    })
}

#[tauri::command(rename_all = "camelCase")]
fn list_recent_commits(
    state: tauri::State<'_, Mutex<AppState>>,
    count: Option<u32>,
) -> Result<Vec<CommitInfo>, String> {
    let state = state.lock().unwrap();
    let repo = &state.repo_path;
    let count = count.unwrap_or(20);

    let output = run_git(
        repo,
        &[
            "log",
            &format!("-{count}"),
            "--format=%H%n%h%n%s%n%an%n%at",
            "--no-merges",
        ],
    )
    .map_err(|e| e.to_string())?;

    let mut commits = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    for chunk in lines.chunks(5) {
        if chunk.len() == 5 {
            commits.push(CommitInfo {
                sha: chunk[0].to_string(),
                short_sha: chunk[1].to_string(),
                message: chunk[2].to_string(),
                author: chunk[3].to_string(),
                timestamp: chunk[4].parse().unwrap_or(0),
            });
        }
    }

    Ok(commits)
}

// =============================================================================
// Commands: Diff operations
// =============================================================================

#[tauri::command(rename_all = "camelCase")]
fn list_diff_files(
    state: tauri::State<'_, Mutex<AppState>>,
    spec: git_diff::DiffSpec,
) -> Result<DiffFilesResponse, String> {
    let state = state.lock().unwrap();
    let repo = &state.repo_path;

    let files = git_diff::list_diff_files(repo, &spec).map_err(|e| e.to_string())?;
    Ok(DiffFilesResponse { files })
}

#[tauri::command(rename_all = "camelCase")]
fn get_file_diff(
    state: tauri::State<'_, Mutex<AppState>>,
    spec: git_diff::DiffSpec,
    path: String,
) -> Result<git_diff::FileDiff, String> {
    let state = state.lock().unwrap();
    let repo = &state.repo_path;
    let file_path = Path::new(&path);

    git_diff::get_file_diff(repo, &spec, file_path).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn get_file_at_ref(
    state: tauri::State<'_, Mutex<AppState>>,
    ref_name: String,
    path: String,
) -> Result<git_diff::File, String> {
    let state = state.lock().unwrap();
    let repo = &state.repo_path;

    git_diff::get_file_at_ref(repo, &ref_name, &path).map_err(|e| e.to_string())
}

// =============================================================================
// Commands: Launch args
// =============================================================================

#[tauri::command(rename_all = "camelCase")]
fn get_launch_args(state: tauri::State<'_, Mutex<AppState>>) -> LaunchArgs {
    let state = state.lock().unwrap();

    let args: Vec<String> = std::env::args().collect();
    let mut mode: Option<String> = None;
    let mut commit: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--staged" | "-s" => mode = Some("staged".to_string()),
            "--branch" | "-b" => mode = Some("branch".to_string()),
            "--commit" | "-c" => {
                mode = Some("commit".to_string());
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    i += 1;
                    commit = Some(args[i].clone());
                }
            }
            "--all" | "-a" => mode = Some("all".to_string()),
            _ => {}
        }
        i += 1;
    }

    LaunchArgs {
        repo_path: state.repo_path.display().to_string(),
        mode,
        commit,
    }
}

// =============================================================================
// Commands: Repo selection
// =============================================================================

#[tauri::command(rename_all = "camelCase")]
fn set_repo_path(state: tauri::State<'_, Mutex<AppState>>, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.join(".git").exists() && run_git(&p, &["rev-parse", "--git-dir"]).is_err() {
        return Err(format!("{path} is not a git repository"));
    }
    let mut s = state.lock().unwrap();
    s.repo_path = p;
    Ok(())
}

// =============================================================================
// Commands: Directory browsing
// =============================================================================

/// List contents of a directory.
/// Returns directories first (sorted), then files (sorted).
/// Hidden files (starting with .) are excluded.
#[tauri::command(rename_all = "camelCase")]
fn list_directory(path: String) -> Result<Vec<DirEntry>, String> {
    let dir = Path::new(&path);

    if !dir.exists() {
        return Err(format!("Directory does not exist: {path}"));
    }
    if !dir.is_dir() {
        return Err(format!("Not a directory: {path}"));
    }

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {e}"))?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }

        let entry_path = entry.path();
        let is_dir = entry_path.is_dir();
        let is_repo = is_dir && entry_path.join(".git").exists();

        let item = DirEntry {
            name,
            path: entry_path.to_string_lossy().to_string(),
            is_dir,
            is_repo,
        };

        if is_dir {
            dirs.push(item);
        } else {
            files.push(item);
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    dirs.extend(files);
    Ok(dirs)
}

/// Folders to skip during search.
const SKIP_FOLDERS: &[&str] = &[
    "Library",
    "Applications",
    "System",
    "Volumes",
    "cores",
    "private",
    "node_modules",
    "target",
    "build",
    "dist",
    "vendor",
    ".git",
    "__pycache__",
    "venv",
    ".venv",
    "env",
    ".cargo",
    ".rustup",
    ".npm",
    ".cache",
    "Caches",
    "Movies",
    "Music",
    "Pictures",
    "Photos Library.photoslibrary",
];

/// Common development folder names.
const DEV_FOLDERS: &[&str] = &[
    "dev",
    "projects",
    "code",
    "repos",
    "src",
    "workspace",
    "work",
    "github",
    "gitlab",
    "Development",
    "Documents",
    "Desktop",
];

/// Search for git repositories matching a query.
#[tauri::command(rename_all = "camelCase")]
fn search_directories(
    path: String,
    query: String,
    max_depth: Option<u32>,
    limit: Option<usize>,
) -> Result<Vec<DirEntry>, String> {
    let dir = Path::new(&path);
    let max_depth = max_depth.unwrap_or(6);
    let limit = limit.unwrap_or(20);
    let query_lower = query.to_lowercase();

    if !dir.exists() || !dir.is_dir() {
        return Err(format!("Invalid directory: {path}"));
    }

    let mut results = Vec::new();
    let collect_limit = limit * 3;

    let home_dir = dirs::home_dir();
    let is_home = home_dir.as_ref().is_some_and(|h| h == dir);

    if is_home {
        for dev_folder in DEV_FOLDERS {
            let dev_path = dir.join(dev_folder);
            if dev_path.exists() && dev_path.is_dir() {
                search_repos_recursive(
                    &dev_path,
                    &query_lower,
                    0,
                    max_depth,
                    &mut results,
                    collect_limit,
                );
                if results.len() >= collect_limit {
                    break;
                }
            }
        }
    } else {
        search_repos_recursive(dir, &query_lower, 0, max_depth, &mut results, collect_limit);
    }

    results.sort_by(|a, b| {
        let a_exact = a.name.to_lowercase() == query_lower;
        let b_exact = b.name.to_lowercase() == query_lower;
        if a_exact != b_exact {
            return b_exact.cmp(&a_exact);
        }
        let a_depth = a.path.matches('/').count();
        let b_depth = b.path.matches('/').count();
        a_depth.cmp(&b_depth)
    });
    results.truncate(limit);

    Ok(results)
}

fn search_repos_recursive(
    dir: &Path,
    query: &str,
    depth: u32,
    max_depth: u32,
    results: &mut Vec<DirEntry>,
    limit: usize,
) -> bool {
    if depth > max_depth || results.len() >= limit {
        return results.len() >= limit;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if SKIP_FOLDERS.contains(&name.as_str()) {
            continue;
        }

        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        let is_repo = entry_path.join(".git").exists();

        if is_repo {
            let name_lower = name.to_lowercase();
            if query.is_empty() || name_lower.starts_with(query) || name_lower.contains(query) {
                results.push(DirEntry {
                    name: name.clone(),
                    path: entry_path.to_string_lossy().to_string(),
                    is_dir: true,
                    is_repo: true,
                });
                if results.len() >= limit {
                    return true;
                }
            }
        } else if search_repos_recursive(&entry_path, query, depth + 1, max_depth, results, limit) {
            return true;
        }
    }

    false
}

/// Get the user's home directory.
#[tauri::command]
fn get_home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine home directory".to_string())
}

fn differ_data_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".differ"))
}

fn preferences_store_path_buf() -> Option<PathBuf> {
    differ_data_dir().map(|d| d.join("preferences.json"))
}

fn migrate_legacy_preferences_file(current_app_data_dir: Option<PathBuf>) {
    let Some(target_path) = preferences_store_path_buf() else {
        eprintln!("Cannot determine preferences path, skipping preferences migration");
        return;
    };

    if target_path.exists() {
        return;
    }

    if let Some(parent) = target_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "Cannot create preferences directory {}: {e}",
                parent.display()
            );
            return;
        }
    }

    let Some(target_dir) = target_path.parent() else {
        return;
    };

    let mut legacy_dirs: Vec<PathBuf> = Vec::new();
    for candidate in [
        dirs::home_dir().map(|d| d.join(".staged")),
        dirs::data_dir().map(|d| d.join("com.staged.app")),
        dirs::data_dir().map(|d| d.join("staged")),
        dirs::home_dir().map(|d| d.join(".mark")),
        dirs::data_dir().map(|d| d.join("com.mark.app")),
        current_app_data_dir,
    ]
    .into_iter()
    .flatten()
    {
        if candidate != target_dir && !legacy_dirs.contains(&candidate) {
            legacy_dirs.push(candidate);
        }
    }

    for old_dir in legacy_dirs {
        let old_path = old_dir.join("preferences.json");
        if old_path == target_path || !old_path.exists() {
            continue;
        }

        eprintln!(
            "Migrating preferences from {} to {}",
            old_path.display(),
            target_path.display()
        );

        if let Err(copy_err) = std::fs::copy(&old_path, &target_path) {
            eprintln!(
                "Failed to copy preferences {} -> {}: {copy_err}",
                old_path.display(),
                target_path.display()
            );
            continue;
        }

        break;
    }
}

/// Return the absolute path for the Differ preferences store file.
#[tauri::command]
fn preferences_store_path() -> Result<String, String> {
    preferences_store_path_buf()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine preferences store path".to_string())
}

// =============================================================================
// Commands: Recent repos (Spotlight)
// =============================================================================

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

#[tauri::command(rename_all = "camelCase")]
async fn find_recent_repos(
    hours_ago: Option<u32>,
    limit: Option<usize>,
) -> Result<Vec<RecentRepo>, String> {
    // Spawn on a blocking thread so mdfind doesn't block the main/command thread
    tauri::async_runtime::spawn_blocking(move || find_recent_repos_sync(hours_ago, limit))
        .await
        .map_err(|e| e.to_string())
}

fn find_recent_repos_sync(hours_ago: Option<u32>, limit: Option<usize>) -> Vec<RecentRepo> {
    let hours_ago = hours_ago.unwrap_or(24);
    let limit = limit.unwrap_or(10);

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

    let mut seen_repos: HashSet<PathBuf> = HashSet::new();
    let mut repos: Vec<RecentRepo> = Vec::new();

    for file in files {
        if EXCLUDE_PATTERNS.iter().any(|p| file.contains(p)) {
            continue;
        }

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

    repos
}

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
    let files: Vec<String> = stdout
        .lines()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    Some(files)
}

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

// =============================================================================
// Helpers
// =============================================================================

fn run_git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(["-C", &repo.display().to_string()])
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.into_owned());
    }

    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

// =============================================================================
// Commands: System fonts
// =============================================================================

#[cfg(target_os = "macos")]
#[tauri::command(rename_all = "camelCase")]
fn list_system_fonts() -> Vec<String> {
    use core_text::font_collection::create_for_all_families;

    let collection = create_for_all_families();
    let descriptors = collection.get_descriptors();
    let mut families: Vec<String> = Vec::new();

    if let Some(descs) = descriptors {
        for i in 0..descs.len() {
            if let Some(desc) = descs.get(i) {
                let name = desc.family_name();
                if !name.starts_with('.') {
                    families.push(name);
                }
            }
        }
    }

    families.sort_by_key(|a| a.to_lowercase());
    families.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    families
}

#[cfg(not(target_os = "macos"))]
#[tauri::command(rename_all = "camelCase")]
fn list_system_fonts() -> Vec<String> {
    Vec::new()
}

fn resolve_repo_path() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    let mut iter = args.iter().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--commit" | "-c" => {
                iter.next();
            }
            s if s.starts_with('-') => {}
            path => {
                let p = PathBuf::from(path);
                if p.exists() {
                    return std::fs::canonicalize(&p).unwrap_or(p);
                }
            }
        }
    }

    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

// =============================================================================
// App entry point
// =============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let repo_path = resolve_repo_path();

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            migrate_legacy_preferences_file(app.path().app_data_dir().ok());
            Ok(())
        })
        .manage(Mutex::new(AppState { repo_path }))
        .invoke_handler(tauri::generate_handler![
            get_repo_info,
            list_recent_commits,
            list_diff_files,
            get_file_diff,
            get_file_at_ref,
            get_launch_args,
            set_repo_path,
            list_directory,
            search_directories,
            get_home_dir,
            preferences_store_path,
            find_recent_repos,
            list_system_fonts,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
