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

    if branch.branch_type == crate::store::BranchType::Remote {
        let workspace_name = branch
            .workspace_name
            .clone()
            .ok_or_else(|| format!("Branch has no workspace name: {branch_id}"))?;
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        return Ok(BranchDiffContext {
            base_branch: branch.base_branch,
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
        base_branch: branch.base_branch,
        worktree_path: Some(workdir.path),
        workspace_name: None,
        repo_subpath: None,
    })
}

fn run_remote_git(ctx: &BranchDiffContext, args: &[&str]) -> Result<String, String> {
    let workspace = ctx
        .workspace_name
        .as_deref()
        .ok_or("Missing remote workspace context")?;
    branches::run_workspace_git(workspace, ctx.repo_subpath.as_deref(), args)
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
            let spec = git::DiffSpec {
                base: git::GitRef::MergeBaseOf([base_branch.to_string(), resolved_sha.clone()]),
                head: git::GitRef::Rev(resolved_sha.clone()),
            };
            Ok((spec, resolved_sha))
        }
    }
}

/// Build explicit base/head refs for a remote branch diff.
///
/// Returns `(base_sha, head_sha, resolved_sha)`.
fn build_remote_diff_refs(
    ctx: &BranchDiffContext,
    commit_sha: Option<&str>,
    scope: &str,
) -> Result<(String, String, String), String> {
    match scope {
        "commit" => {
            let head = commit_sha
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("commit_sha required for commit scope")?
                .to_string();
            let parent = run_remote_git(ctx, &["rev-parse", &format!("{head}^")])
                .map(|s| s.trim().to_string())
                .map_err(|_| format!("No parent commit for {head}"))?;
            Ok((parent, head.clone(), head))
        }
        _ => {
            let head = match commit_sha.map(str::trim).filter(|s| !s.is_empty()) {
                Some(sha) => sha.to_string(),
                None => run_remote_git(ctx, &["rev-parse", "HEAD"])?
                    .trim()
                    .to_string(),
            };
            let base = run_remote_git(ctx, &["merge-base", &ctx.base_branch, &head])?
                .trim()
                .to_string();
            Ok((base, head.clone(), head))
        }
    }
}

/// Parse `git diff --name-status -z` output.
fn parse_name_status_z(output: &str) -> Vec<git::FileDiffSummary> {
    let mut results = Vec::new();
    let mut parts = output.split('\0').peekable();

    while let Some(status) = parts.next() {
        if status.is_empty() {
            continue;
        }

        let status_char = status.chars().next().unwrap_or(' ');

        match status_char {
            'A' => {
                if let Some(path) = parts.next() {
                    results.push(git::FileDiffSummary {
                        before: None,
                        after: Some(path.into()),
                    });
                }
            }
            'D' => {
                if let Some(path) = parts.next() {
                    results.push(git::FileDiffSummary {
                        before: Some(path.into()),
                        after: None,
                    });
                }
            }
            'M' | 'T' => {
                if let Some(path) = parts.next() {
                    results.push(git::FileDiffSummary {
                        before: Some(path.into()),
                        after: Some(path.into()),
                    });
                }
            }
            'R' | 'C' => {
                if let (Some(old), Some(new)) = (parts.next(), parts.next()) {
                    results.push(git::FileDiffSummary {
                        before: Some(old.into()),
                        after: Some(new.into()),
                    });
                }
            }
            _ => {
                parts.next();
            }
        }
    }

    results
}

#[derive(Debug, Clone, Copy)]
struct RemoteHunk {
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

fn parse_unified_hunks(diff_text: &str) -> Vec<RemoteHunk> {
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

fn file_content_from_text(text: &str) -> git::FileContent {
    if text.as_bytes()[..text.len().min(8192)].contains(&0) {
        return git::FileContent::Binary;
    }
    git::FileContent::Text {
        lines: text.lines().map(|line| line.to_string()).collect(),
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

fn is_utf8_parse_error(msg: &str) -> bool {
    msg.to_lowercase()
        .contains("invalid utf-8 in sq blox output")
}

fn load_remote_file_at_ref(
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

    match run_remote_git(ctx, &["show", &spec]) {
        Ok(content) => Ok(Some(git::File {
            path: path.to_string(),
            content: file_content_from_text(&content),
        })),
        Err(e) if is_utf8_parse_error(&e) => Ok(Some(git::File {
            path: path.to_string(),
            content: git::FileContent::Binary,
        })),
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

fn compute_remote_alignments(
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
    let store = crate::get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    let (files, resolved_sha) = if let Some(worktree_path) = ctx.worktree_path.as_deref() {
        let worktree = Path::new(worktree_path);
        let (spec, resolved_sha) =
            build_diff_spec(worktree, &ctx.base_branch, commit_sha.as_deref(), &scope)?;
        let files = git::list_diff_files(worktree, &spec).map_err(|e| e.to_string())?;
        (files, resolved_sha)
    } else {
        let (base, head, resolved_sha) =
            build_remote_diff_refs(&ctx, commit_sha.as_deref(), &scope)?;
        let output = run_remote_git(&ctx, &["diff", "--name-status", "-z", &base, &head])?;
        (parse_name_status_z(&output), resolved_sha)
    };

    Ok(DiffFilesResponse {
        commit_sha: resolved_sha,
        files,
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
    let store = crate::get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    if let Some(worktree_path) = ctx.worktree_path.as_deref() {
        let worktree = Path::new(worktree_path);
        let (spec, _) = build_diff_spec(worktree, &ctx.base_branch, Some(&commit_sha), &scope)?;
        let file_path = Path::new(&path);
        return git::get_file_diff(worktree, &spec, file_path).map_err(|e| e.to_string());
    }

    let (base, head, _) = build_remote_diff_refs(&ctx, Some(&commit_sha), &scope)?;
    let before = load_remote_file_at_ref(&ctx, &base, &path)?;
    let after = load_remote_file_at_ref(&ctx, &head, &path)?;
    let patch = run_remote_git(
        &ctx,
        &[
            "-c",
            "color.ui=never",
            "diff",
            "--unified=0",
            &base,
            &head,
            "--",
            &path,
        ],
    )?;
    let hunks = parse_unified_hunks(&patch);
    let alignments = compute_remote_alignments(&hunks, &before, &after);

    Ok(git::FileDiff {
        before,
        after,
        alignments,
    })
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
