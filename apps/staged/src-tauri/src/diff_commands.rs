//! Diff commands — computing file diffs for branches (local and remote).

use crate::branches;
use crate::git;
use crate::store::Store;
use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Context needed to compute diffs for a branch.
pub(crate) struct BranchDiffContext {
    pub base_branch: String,
    pub project_id: String,
    pub project_location: crate::store::ProjectLocation,
    pub worktree_path: Option<String>,
    pub workspace_name: Option<String>,
    pub repo_subpath: Option<String>,
}

/// Resolve the worktree path and base branch for a given branch.
pub(crate) fn resolve_branch_context(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<BranchDiffContext, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    if branch.branch_type == crate::store::BranchType::Remote {
        let workspace_name = branch
            .workspace_name
            .clone()
            .ok_or_else(|| format!("Branch has no workspace name: {branch_id}"))?;

        // Use the repo root (clone dir only) so all paths are consistently
        // repo-root-relative — git diff pathspecs, git show tree paths, etc.
        let repo_subpath = branches::resolve_branch_clone_dir(store, &branch)?;

        return Ok(BranchDiffContext {
            base_branch: git::origin_ref_for_branch(&branch.base_branch),
            project_id: project.id,
            project_location: project.location,
            worktree_path: None,
            workspace_name: Some(workspace_name),
            repo_subpath,
        });
    }

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    Ok(BranchDiffContext {
        base_branch: git::origin_ref_for_branch(&branch.base_branch),
        project_id: project.id,
        project_location: project.location,
        worktree_path: Some(workdir.path),
        workspace_name: None,
        repo_subpath: None,
    })
}

pub(crate) fn run_remote_git(ctx: &BranchDiffContext, args: &[&str]) -> Result<String, String> {
    let workspace = ctx
        .workspace_name
        .as_deref()
        .ok_or("Missing remote workspace context")?;
    branches::run_workspace_git(workspace, ctx.repo_subpath.as_deref(), args)
        .map_err(|e| e.to_string())
}

fn run_remote_git_bytes(ctx: &BranchDiffContext, args: &[&str]) -> Result<Vec<u8>, String> {
    let workspace = ctx
        .workspace_name
        .as_deref()
        .ok_or("Missing remote workspace context")?;
    branches::run_workspace_git_bytes(workspace, ctx.repo_subpath.as_deref(), args)
        .map_err(|e| e.to_string())
}

/// Build a DiffSpec for a branch diff.
///
/// - Branch scope with no commit_sha: merge-base(base, tip)..tip
/// - Branch scope with commit_sha: merge-base(base, sha)..sha
/// - Commit scope: sha~1..sha
fn build_diff_spec(
    worktree: &Path,
    base_branch: &str,
    commit_sha: Option<&str>,
    scope: &str,
) -> Result<(git::DiffSpec, String), String> {
    match scope {
        "commit" => {
            let sha = commit_sha.ok_or("commit_sha required for commit scope")?;
            let parent = git::get_parent_commit(worktree, sha)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("No parent commit for {sha}"))?;
            let spec = git::DiffSpec {
                base: git::GitRef::Rev(parent),
                head: git::GitRef::Rev(sha.to_string()),
            };
            Ok((spec, sha.to_string()))
        }
        _ => {
            let resolved_sha = match commit_sha {
                Some(sha) => sha.to_string(),
                None => git::get_head_sha(worktree).map_err(|e| e.to_string())?,
            };
            let base_ref = git::origin_ref_for_branch(base_branch);
            let spec = git::DiffSpec {
                base: git::GitRef::MergeBaseOf([base_ref, resolved_sha.clone()]),
                head: git::GitRef::Rev(resolved_sha.clone()),
            };
            Ok((spec, resolved_sha))
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RemoteHunk {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
}

fn parse_hunk_range(raw: &str) -> Option<(u32, u32)> {
    let (start_raw, lines_raw) = match raw.split_once(',') {
        Some((start, lines)) => (start, lines),
        None => (raw, "1"),
    };
    let start = start_raw.trim().parse::<u32>().ok()?;
    let lines = lines_raw.trim().parse::<u32>().ok()?;
    let start_zero = if start == 0 { 0 } else { start - 1 };
    Some((start_zero, lines))
}

pub(crate) fn parse_unified_hunks(diff_text: &str) -> Vec<RemoteHunk> {
    let mut hunks = Vec::new();

    for line in diff_text.lines() {
        if !line.starts_with("@@ -") {
            continue;
        }
        let Some(after_minus) = line.strip_prefix("@@ -") else {
            continue;
        };
        let Some((old_part, rest)) = after_minus.split_once(" +") else {
            continue;
        };
        let Some((new_part, _)) = rest.split_once(" @@") else {
            continue;
        };

        let Some((old_start, old_lines)) = parse_hunk_range(old_part) else {
            continue;
        };
        let Some((new_start, new_lines)) = parse_hunk_range(new_part) else {
            continue;
        };

        hunks.push(RemoteHunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
        });
    }

    hunks
}

pub(crate) fn file_content_from_bytes(bytes: &[u8], path: &str) -> git::FileContent {
    let check_len = bytes.len().min(8192);
    if bytes[..check_len].contains(&0) {
        return file_content_binary_or_image(bytes, path);
    }
    let text = String::from_utf8_lossy(bytes);
    git::FileContent::Text {
        lines: text.lines().map(|line| line.to_string()).collect(),
    }
}

/// For binary content in the remote path, try to produce an ImageBase64 variant.
fn file_content_binary_or_image(bytes: &[u8], path: &str) -> git::FileContent {
    if bytes.len() > git::IMAGE_PREVIEW_MAX_BYTES {
        return git::FileContent::Binary;
    }

    let file_path = std::path::Path::new(path);
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    let mime = match ext.as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    };

    if let Some(mime) = mime {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        git::FileContent::ImageBase64 {
            mime_type: mime.to_string(),
            data,
        }
    } else {
        git::FileContent::Binary
    }
}

fn is_missing_object_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("not a valid object name")
        || lower.contains("pathspec")
        || lower.contains("does not exist")
        || lower.contains("exists on disk, but not in")
        || lower.contains("path '")
}

pub(crate) fn load_remote_file_at_ref(
    ctx: &BranchDiffContext,
    ref_name: &str,
    path: &str,
) -> Result<Option<git::File>, String> {
    let spec = format!("{ref_name}:{path}");

    match run_remote_git(ctx, &["cat-file", "-e", &spec]) {
        Ok(_) => {}
        Err(e) if is_missing_object_error(&e) => return Ok(None),
        Err(e) => return Err(e),
    }

    match run_remote_git_bytes(ctx, &["show", &spec]) {
        Ok(bytes) => {
            let content = file_content_from_bytes(&bytes, path);
            Ok(Some(git::File {
                path: path.to_string(),
                content,
            }))
        }
        Err(e) => Err(e),
    }
}

fn remote_file_len(file: &Option<git::File>) -> u32 {
    match file {
        Some(git::File {
            content: git::FileContent::Text { lines },
            ..
        }) => lines.len() as u32,
        _ => 0,
    }
}

pub(crate) fn compute_remote_alignments(
    hunks: &[RemoteHunk],
    before: &Option<git::File>,
    after: &Option<git::File>,
) -> Vec<git::Alignment> {
    let before_len = remote_file_len(before);
    let after_len = remote_file_len(after);

    if before_len == 0 && after_len == 0 {
        return vec![];
    }

    if hunks.is_empty() {
        if before_len == 0 {
            return vec![git::Alignment {
                before: git::Span::new(0, 0),
                after: git::Span::new(0, after_len),
                changed: true,
            }];
        }
        if after_len == 0 {
            return vec![git::Alignment {
                before: git::Span::new(0, before_len),
                after: git::Span::new(0, 0),
                changed: true,
            }];
        }
        return vec![git::Alignment {
            before: git::Span::new(0, before_len),
            after: git::Span::new(0, after_len),
            changed: false,
        }];
    }

    let mut alignments = Vec::new();
    let mut before_pos = 0u32;
    let mut after_pos = 0u32;

    for hunk in hunks {
        if before_pos < hunk.old_start || after_pos < hunk.new_start {
            alignments.push(git::Alignment {
                before: git::Span::new(before_pos, hunk.old_start),
                after: git::Span::new(after_pos, hunk.new_start),
                changed: false,
            });
        }

        let before_end = hunk.old_start + hunk.old_lines;
        let after_end = hunk.new_start + hunk.new_lines;

        alignments.push(git::Alignment {
            before: git::Span::new(hunk.old_start, before_end),
            after: git::Span::new(hunk.new_start, after_end),
            changed: true,
        });

        before_pos = before_end;
        after_pos = after_end;
    }

    if before_pos < before_len || after_pos < after_len {
        alignments.push(git::Alignment {
            before: git::Span::new(before_pos, before_len),
            after: git::Span::new(after_pos, after_len),
            changed: false,
        });
    }

    alignments
}

/// Response from get_diff_files including the resolved commit SHA.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFilesResponse {
    /// The resolved commit SHA (tip for branch scope, or the passed-in SHA).
    commit_sha: String,
    /// Changed files in the diff.
    files: Vec<git::FileDiffSummary>,
}

/// Resolve head SHA, gather commit SHAs, and run `collect_and_cache` to ensure
/// the diff cache is populated for the given branch/scope. Returns the full
/// collection result so callers can extract whatever they need.
fn ensure_cache_populated(
    ctx: &BranchDiffContext,
    store: &Arc<Store>,
    branch_id: &str,
    scope: &str,
    commit_sha: Option<&str>,
) -> Result<crate::diff_cache::CollectedDiffs, String> {
    let workspace_name = ctx
        .workspace_name
        .as_deref()
        .ok_or("Missing remote workspace context")?;

    let latest_sha = store
        .list_commits_for_branch(branch_id)
        .ok()
        .and_then(|commits| commits.into_iter().rev().find_map(|c| c.sha));

    let head_sha = commit_sha
        .filter(|s| !s.is_empty() && scope == "branch")
        .or(latest_sha.as_deref())
        .map(|s| Ok(s.to_string()))
        .unwrap_or_else(|| {
            run_remote_git(ctx, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string())
        })?;

    let mut all_commit_shas: Vec<String> = store
        .list_commits_for_branch(branch_id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| c.sha)
        .collect();

    if scope == "commit" {
        if let Some(sha) = commit_sha {
            if !all_commit_shas.contains(&sha.to_string()) {
                all_commit_shas.push(sha.to_string());
            }
        }
    }

    crate::diff_cache::collect_and_cache(
        ctx.project_location,
        &ctx.project_id,
        branch_id,
        workspace_name,
        ctx.repo_subpath.as_deref(),
        &ctx.base_branch,
        &head_sha,
        &all_commit_shas,
    )
    .map_err(|e| e.to_string())
}

/// List files changed in a branch or commit diff.
///
/// For branch scope: merge-base(base, tip)..tip
/// For commit scope: parent..sha
///
/// `commit_sha` is optional for branch scope (resolves to current tip).
#[tauri::command(rename_all = "camelCase")]
pub async fn get_diff_files(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: Option<String>,
    scope: String,
) -> Result<DiffFilesResponse, String> {
    let start = std::time::Instant::now();
    log::info!("get_diff_files: branch_id={branch_id} scope={scope} commit_sha={commit_sha:?}");
    let store = crate::get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    if let Some(worktree_path) = ctx.worktree_path.as_deref() {
        let worktree = Path::new(worktree_path);
        let (spec, resolved_sha) =
            build_diff_spec(worktree, &ctx.base_branch, commit_sha.as_deref(), &scope)?;
        let t0 = std::time::Instant::now();
        let files = git::list_diff_files(worktree, &spec).map_err(|e| e.to_string())?;
        log::info!(
            "get_diff_files: local list_diff_files returned {} files in {:?} (total {:?})",
            files.len(),
            t0.elapsed(),
            start.elapsed()
        );
        return Ok(DiffFilesResponse {
            commit_sha: resolved_sha,
            files,
        });
    }

    // Remote branch — check cache, then collect on miss.
    let latest_sha = store
        .list_commits_for_branch(&branch_id)
        .ok()
        .and_then(|commits| commits.into_iter().rev().find_map(|c| c.sha));

    if scope == "branch" {
        if let Some(ref sha) = latest_sha {
            let check_sha = commit_sha.as_deref().unwrap_or(sha.as_str());
            if check_sha == sha {
                if let Some(cached) = crate::diff_cache::load_cache_index(
                    ctx.project_location,
                    &ctx.project_id,
                    &branch_id,
                    sha,
                ) {
                    return Ok(DiffFilesResponse {
                        commit_sha: sha.clone(),
                        files: cached.files,
                    });
                }
            }
        }
    }
    if scope == "commit" {
        if let Some(ref sha) = commit_sha {
            if let Some(cached) = crate::diff_cache::load_commit_index(
                ctx.project_location,
                &ctx.project_id,
                &branch_id,
                sha,
            ) {
                return Ok(DiffFilesResponse {
                    commit_sha: sha.clone(),
                    files: cached.files,
                });
            }
        }
    }

    let (index, _, commit_results) =
        ensure_cache_populated(&ctx, &store, &branch_id, &scope, commit_sha.as_deref())?;

    if scope == "commit" {
        let sha = commit_sha.ok_or("commit_sha required for commit scope")?;
        let files = commit_results
            .iter()
            .find(|(ci, _)| ci.sha == sha)
            .map(|(ci, _)| ci.files.clone())
            .unwrap_or_default();
        return Ok(DiffFilesResponse {
            commit_sha: sha,
            files,
        });
    }

    Ok(DiffFilesResponse {
        commit_sha: index.head_sha,
        files: index.files,
    })
}

/// Get the full diff content for a single file.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_file_diff(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    scope: String,
    path: String,
) -> Result<git::FileDiff, String> {
    let start = std::time::Instant::now();
    log::info!("get_file_diff: path={path} scope={scope}");
    let store = crate::get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    if let Some(worktree_path) = ctx.worktree_path.as_deref() {
        let worktree = Path::new(worktree_path);
        let (spec, _) = build_diff_spec(worktree, &ctx.base_branch, Some(&commit_sha), &scope)?;
        let file_path = Path::new(&path);
        let result = git::get_file_diff(worktree, &spec, file_path).map_err(|e| e.to_string())?;
        fn file_stats(f: &Option<git::File>) -> (usize, usize) {
            match f {
                Some(git::File {
                    content: git::FileContent::Text { lines },
                    ..
                }) => {
                    let max_len = lines.iter().map(|l| l.len()).max().unwrap_or(0);
                    (lines.len(), max_len)
                }
                _ => (0, 0),
            }
        }
        let (before_lines, before_max) = file_stats(&result.before);
        let (after_lines, after_max) = file_stats(&result.after);
        log::info!(
            "get_file_diff: path={path} done in {:?} before={before_lines} lines (max {before_max} chars) after={after_lines} lines (max {after_max} chars) alignments={}",
            start.elapsed(),
            result.alignments.len()
        );
        return Ok(result);
    }

    // Check cache for branch-scope diffs.
    if scope == "branch" {
        if let Some(file_diff) = crate::diff_cache::load_cached_file_diff(
            ctx.project_location,
            &ctx.project_id,
            &branch_id,
            &commit_sha,
            &path,
        ) {
            return Ok(file_diff);
        }
    }

    // Check cache for commit-scope diffs.
    if scope == "commit" {
        if let Some(file_diff) = crate::diff_cache::load_commit_file_diff(
            ctx.project_location,
            &ctx.project_id,
            &branch_id,
            &commit_sha,
            &path,
        ) {
            return Ok(file_diff);
        }
    }

    let (_, branch_file_diffs, commit_results) =
        ensure_cache_populated(&ctx, &store, &branch_id, &scope, Some(&commit_sha))?;

    if scope == "commit" {
        if let Some(diff) = commit_results
            .iter()
            .find(|(ci, _)| ci.sha == commit_sha)
            .and_then(|(_, diffs)| diffs.get(&path))
        {
            return Ok(diff.clone());
        }
    } else if let Some(diff) = branch_file_diffs.get(&path) {
        return Ok(diff.clone());
    }

    Err(format!("File not found in diff: {path}"))
}

/// Get file content at a specific ref (for reference files).
#[tauri::command(rename_all = "camelCase")]
pub async fn get_file_at_ref(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    ref_name: String,
    path: String,
) -> Result<git::File, String> {
    let store = crate::get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    if let Some(worktree_path) = ctx.worktree_path.as_deref() {
        let worktree = Path::new(worktree_path);
        return git::get_file_at_ref(worktree, &ref_name, &path).map_err(|e| e.to_string());
    }

    let effective_ref = if ref_name == git::WORKDIR {
        "HEAD"
    } else {
        ref_name.as_str()
    };
    load_remote_file_at_ref(&ctx, effective_ref, &path)?
        .ok_or_else(|| format!("File not found: {path}"))
}
