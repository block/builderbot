//! Staged — standalone diff viewer.
//!
//! A focused diff viewer that opens a git repository and shows diffs
//! using the shared git-diff crate and @builderbot/diff-viewer package.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
// Commands: Dialog
// =============================================================================

#[tauri::command(rename_all = "camelCase")]
async fn open_repo_dialog(
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let path = app.dialog().file().blocking_pick_folder();

    match path {
        Some(folder) => {
            let folder_path = folder.to_string();
            let path = PathBuf::from(&folder_path);
            if !path.join(".git").exists() && run_git(&path, &["rev-parse", "--git-dir"]).is_err() {
                return Err(format!("{folder_path} is not a git repository"));
            }
            let mut s = state.lock().unwrap();
            s.repo_path = path;
            Ok(Some(folder_path))
        }
        None => Ok(None),
    }
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

fn resolve_repo_path() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    let mut iter = args.iter().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--commit" | "-c" => {
                iter.next(); // skip the commit value
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
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(AppState { repo_path }))
        .invoke_handler(tauri::generate_handler![
            get_repo_info,
            list_recent_commits,
            list_diff_files,
            get_file_diff,
            get_file_at_ref,
            get_launch_args,
            open_repo_dialog,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
