//! Timeline — branch timeline construction and related delete commands.

use crate::git;
use crate::session_runner;
use crate::store::{CommentAuthor, Review, Store};
use crate::{
    blox, branches, BranchTimeline, CommitTimelineItem, ImageTimelineItem, NoteTimelineItem,
    ReviewTimelineItem,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

fn build_branch_timeline(store: &Arc<Store>, branch_id: &str) -> Result<BranchTimeline, String> {
    // Get the branch and its workdir for git operations
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?;

    let mut git_state = None;

    // Get commits from git (the source of truth for commit data)
    let mut commits = Vec::new();
    if let Some(ref ws_name) = branch.workspace_name {
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        let cache_key = remote_git_state_cache_key(
            ws_name,
            repo_subpath.as_deref(),
            &branch.branch_name,
            &branch.base_branch,
        );
        git_state = Some(git::compute_branch_git_state(
            &cache_key,
            |args| {
                branches::run_workspace_git(ws_name, repo_subpath.as_deref(), args)
                    .map_err(|e| e.to_string())
            },
            &branch.branch_name,
            &branch.base_branch,
            git::FetchMode::Ttl,
        ));

        let base_ref = git::origin_ref_for_branch(&branch.base_branch);
        // Remote branch: fetch commits via ws_exec.
        // Use merge-base to find the fork point so that only the branch's
        // own commits are shown, even after a rebase or when the base ref
        // has moved forward.
        let range = if let Ok(mb_output) = branches::run_workspace_git(
            ws_name,
            repo_subpath.as_deref(),
            &["merge-base", &base_ref, "HEAD"],
        ) {
            let mb = mb_output.trim().to_string();
            format!("{mb}..HEAD")
        } else {
            // Fallback: if merge-base fails (e.g. shallow clone), use
            // the remote-tracking base ref.
            format!("{base_ref}..HEAD")
        };
        let format_arg = "--format=%H|%h|%s|%an|%ct";
        let output = branches::run_workspace_git(
            ws_name,
            repo_subpath.as_deref(),
            &["log", format_arg, &range],
        )
        .map_err(|e| format!("Failed to load commits from workspace: {e}"))?;
        // git log returns newest-first; parse then assign order so 0 = oldest.
        for line in output.lines() {
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            if parts.len() >= 5 {
                let sha = parts[0].to_string();
                let our_commit = store.get_commit_by_sha(branch_id, &sha).unwrap_or(None);
                let resolved = store.resolve_session_status(
                    our_commit.as_ref().and_then(|c| c.session_id.as_deref()),
                );

                commits.push(CommitTimelineItem {
                    id: our_commit.as_ref().map(|c| c.id.clone()),
                    sha,
                    short_sha: parts[1].to_string(),
                    subject: parts[2].to_string(),
                    author: parts[3].to_string(),
                    timestamp: parts[4].parse().unwrap_or(0),
                    order: 0, // placeholder, assigned below
                    session_id: resolved.session_id,
                    session_status: resolved.status,
                    completion_reason: resolved.completion_reason,
                });
            }
        }
        let len = commits.len() as i64;
        for (i, commit) in commits.iter_mut().enumerate() {
            commit.order = len - 1 - i as i64;
        }
    } else if let Some(ref wd) = workdir {
        // Local branch: fetch commits from the local worktree
        let worktree_path = Path::new(&wd.path);
        if worktree_path.exists() {
            git_state = Some(git::compute_local_branch_git_state(
                worktree_path,
                &branch.branch_name,
                &branch.base_branch,
                git::FetchMode::Ttl,
            ));

            let base_ref = git::origin_ref_for_branch(&branch.base_branch);
            let git_commits =
                git::get_commits_since_base(worktree_path, &base_ref).map_err(|e| {
                    format!("Failed to get commits since base for branch {branch_id}: {e:?}")
                })?;

            // For each git commit, look up our metadata (session linkage)
            for gc in git_commits {
                let our_commit = store.get_commit_by_sha(branch_id, &gc.sha).unwrap_or(None);
                let resolved = store.resolve_session_status(
                    our_commit.as_ref().and_then(|c| c.session_id.as_deref()),
                );

                commits.push(CommitTimelineItem {
                    id: our_commit.as_ref().map(|c| c.id.clone()),
                    sha: gc.sha,
                    short_sha: gc.short_sha,
                    subject: gc.subject,
                    author: gc.author,
                    timestamp: gc.timestamp,
                    order: gc.order,
                    session_id: resolved.session_id,
                    session_status: resolved.status,
                    completion_reason: resolved.completion_reason,
                });
            }
        }
    }

    // Also include pending commits (sha = None, i.e. session in progress)
    let db_commits = store
        .list_commits_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    for dc in db_commits {
        if dc.sha.is_none() {
            let resolved = store.resolve_session_status(dc.session_id.as_deref());

            commits.push(CommitTimelineItem {
                id: Some(dc.id.clone()),
                sha: String::new(),
                short_sha: String::new(),
                subject: resolved
                    .session_id
                    .as_deref()
                    .and_then(|sid| {
                        store
                            .get_session(sid)
                            .ok()
                            .flatten()
                            .map(|s| s.prompt.clone())
                    })
                    .unwrap_or_else(|| "Pending commit".to_string()),
                author: String::new(),
                timestamp: dc.created_at / 1000, // convert ms to seconds
                order: 0, // created_at is ms divided by 1000, so two pending commits in the same second could tie; rare in practice since they're created one at a time
                session_id: resolved.session_id,
                session_status: resolved.status,
                completion_reason: resolved.completion_reason,
            });
        }
    }

    let visible_shas: HashSet<&str> = commits
        .iter()
        .filter(|c| !c.sha.is_empty())
        .map(|c| c.sha.as_str())
        .collect();

    // Get notes
    let db_notes = store
        .list_notes_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    let notes: Vec<NoteTimelineItem> = db_notes
        .into_iter()
        .map(|n| {
            let resolved = store.resolve_session_status(n.session_id.as_deref());
            NoteTimelineItem {
                id: n.id,
                title: n.title,
                content: n.content,
                session_id: resolved.session_id,
                session_status: resolved.status,
                completion_reason: resolved.completion_reason,
                created_at: n.created_at,
                updated_at: n.updated_at,
                completed_at: n.completed_at,
                suggested_next_commit_step: n.suggested_next_commit_step,
                suggested_next_note_step: n.suggested_next_note_step,
            }
        })
        .collect();

    let db_reviews = store
        .list_reviews_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    let reviews: Vec<ReviewTimelineItem> = db_reviews
        .into_iter()
        .filter(|r| review_is_visible_in_timeline(r, &visible_shas))
        .map(|r| {
            let resolved = store.resolve_session_status(r.session_id.as_deref());
            let comment_count = r.comments.len();
            ReviewTimelineItem {
                id: r.id,
                commit_sha: r.commit_sha,
                scope: r.scope.as_str().to_string(),
                session_id: resolved.session_id,
                session_status: resolved.status,
                session_provider: resolved.provider,
                completion_reason: resolved.completion_reason,
                title: r.title,
                comment_count,
                is_auto: r.is_auto,
                created_at: r.created_at,
                updated_at: r.updated_at,
                completed_at: r.completed_at,
            }
        })
        .collect();

    // Get images
    let db_images = store
        .list_images_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    let images: Vec<ImageTimelineItem> = db_images
        .into_iter()
        .map(|img| {
            let resolved = store.resolve_session_status(img.session_id.as_deref());
            ImageTimelineItem {
                id: img.id,
                filename: img.filename,
                mime_type: img.mime_type,
                size_bytes: img.size_bytes,
                session_id: resolved.session_id,
                session_status: resolved.status,
                completion_reason: resolved.completion_reason,
                created_at: img.created_at,
            }
        })
        .collect();

    Ok(BranchTimeline {
        commits,
        notes,
        reviews,
        images,
        git_state,
    })
}

fn remote_git_state_cache_key(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    branch_name: &str,
    base_branch: &str,
) -> String {
    format!(
        "remote:{workspace_name}:{}:{branch_name}:{base_branch}",
        repo_subpath.unwrap_or("")
    )
}

fn review_is_visible_in_timeline(review: &Review, visible_shas: &HashSet<&str>) -> bool {
    review.commit_sha.is_empty()
        || visible_shas.contains(review.commit_sha.as_str())
        || review
            .comments
            .iter()
            .any(|comment| comment.author == CommentAuthor::User)
}

#[tauri::command]
pub async fn get_branch_timeline(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<BranchTimeline, String> {
    let store = crate::get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || build_branch_timeline(&store, &branch_id))
        .await
        .map_err(|e| format!("Timeline task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn pull_branch_ff_only(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || {
        let branch = store
            .get_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

        if let Some(ref ws_name) = branch.workspace_name {
            let repo_subpath = branches::resolve_branch_workspace_subpath(&store, &branch)?;
            let cache_key = remote_git_state_cache_key(
                ws_name,
                repo_subpath.as_deref(),
                &branch.branch_name,
                &branch.base_branch,
            );
            let state = git::compute_branch_git_state(
                &cache_key,
                |args| {
                    branches::run_workspace_git(ws_name, repo_subpath.as_deref(), args)
                        .map_err(|e| e.to_string())
                },
                &branch.branch_name,
                &branch.base_branch,
                git::FetchMode::Force,
            );
            git::ensure_fast_forward_pullable(&state)?;
            branches::run_workspace_git(
                ws_name,
                repo_subpath.as_deref(),
                &["merge", "--ff-only", &state.upstream.r#ref],
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }

        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;
        let worktree = Path::new(&workdir.path);
        let state = git::compute_local_branch_git_state(
            worktree,
            &branch.branch_name,
            &branch.base_branch,
            git::FetchMode::Force,
        );
        git::ensure_fast_forward_pullable(&state)?;
        git::fast_forward_to_ref(worktree, &state.upstream.r#ref).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| format!("Pull task failed: {e}"))?
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeChangesPreview {
    revert_paths: Vec<String>,
    remove_paths: Vec<String>,
    conflicted_paths: Vec<String>,
}

fn preview_from_change_paths(paths: git::WorktreeChangePaths) -> WorktreeChangesPreview {
    WorktreeChangesPreview {
        revert_paths: paths.revert_paths,
        remove_paths: paths.remove_paths,
        conflicted_paths: paths.conflicted_paths,
    }
}

fn ensure_preview_matches(
    changes: &git::WorktreeChangePaths,
    expected: Option<&WorktreeChangesPreview>,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = preview_from_change_paths(changes.clone());
    if &actual == expected {
        Ok(())
    } else {
        Err("Worktree changes changed; review the discard preview again".to_string())
    }
}

fn ensure_worktree_discardable(state: &git::BranchGitState) -> Result<(), String> {
    if state.detached_head {
        return Err("Cannot discard changes while HEAD is detached".to_string());
    }
    if !state.expected_branch_matches {
        let current = state
            .current_branch
            .as_deref()
            .unwrap_or("an unknown branch");
        return Err(format!(
            "Cannot discard changes while checked out on {current}"
        ));
    }
    if state.worktree.conflicted > 0 {
        return Err("Resolve merge conflicts before discarding changes".to_string());
    }
    Ok(())
}

fn remote_worktree_change_paths(
    workspace_name: &str,
    repo_subpath: Option<&str>,
) -> Result<git::WorktreeChangePaths, String> {
    let output = branches::run_workspace_git(
        workspace_name,
        repo_subpath,
        &["status", "--porcelain=1", "-z", "--untracked-files=all"],
    )
    .map_err(|e| e.to_string())?;
    Ok(git::parse_worktree_status_paths(&output))
}

fn discard_remote_worktree_changes(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    changes: &git::WorktreeChangePaths,
) -> Result<(), String> {
    if changes.reset_required {
        branches::run_workspace_git(workspace_name, repo_subpath, &["reset", "--hard", "HEAD"])
            .map_err(|e| e.to_string())?;
    }

    if !changes.remove_paths.is_empty() {
        let pathspecs = changes
            .remove_paths
            .iter()
            .map(|path| format!(":(literal){path}"))
            .collect::<Vec<_>>();
        let mut args = vec!["clean", "-fd", "--"];
        args.extend(pathspecs.iter().map(String::as_str));
        branches::run_workspace_git(workspace_name, repo_subpath, &args)
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_worktree_changes_preview(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<WorktreeChangesPreview, String> {
    let store = crate::get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || {
        let branch = store
            .get_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

        if let Some(ref ws_name) = branch.workspace_name {
            let repo_subpath = branches::resolve_branch_workspace_subpath(&store, &branch)?;
            return remote_worktree_change_paths(ws_name, repo_subpath.as_deref())
                .map(preview_from_change_paths);
        }

        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;
        let paths =
            git::list_worktree_change_paths(Path::new(&workdir.path)).map_err(|e| e.to_string())?;
        Ok(preview_from_change_paths(paths))
    })
    .await
    .map_err(|e| format!("Worktree preview task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn discard_worktree_changes(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    expected_preview: Option<WorktreeChangesPreview>,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || {
        let branch = store
            .get_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

        if let Some(ref ws_name) = branch.workspace_name {
            let repo_subpath = branches::resolve_branch_workspace_subpath(&store, &branch)?;
            let cache_key = remote_git_state_cache_key(
                ws_name,
                repo_subpath.as_deref(),
                &branch.branch_name,
                &branch.base_branch,
            );
            let state = git::compute_branch_git_state(
                &cache_key,
                |args| {
                    branches::run_workspace_git(ws_name, repo_subpath.as_deref(), args)
                        .map_err(|e| e.to_string())
                },
                &branch.branch_name,
                &branch.base_branch,
                git::FetchMode::Never,
            );
            ensure_worktree_discardable(&state)?;
            let changes = remote_worktree_change_paths(ws_name, repo_subpath.as_deref())?;
            ensure_preview_matches(&changes, expected_preview.as_ref())?;
            if changes.is_empty() {
                return Ok(());
            }
            if !changes.conflicted_paths.is_empty() {
                return Err("Resolve merge conflicts before discarding changes".to_string());
            }
            return discard_remote_worktree_changes(ws_name, repo_subpath.as_deref(), &changes);
        }

        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;
        let worktree = Path::new(&workdir.path);
        let state = git::compute_local_branch_git_state(
            worktree,
            &branch.branch_name,
            &branch.base_branch,
            git::FetchMode::Never,
        );
        ensure_worktree_discardable(&state)?;
        let changes = git::list_worktree_change_paths(worktree).map_err(|e| e.to_string())?;
        ensure_preview_matches(&changes, expected_preview.as_ref())?;
        if changes.is_empty() {
            return Ok(());
        }
        if !changes.conflicted_paths.is_empty() {
            return Err("Resolve merge conflicts before discarding changes".to_string());
        }
        git::discard_worktree_changes(worktree, &changes).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| format!("Discard task failed: {e}"))?
}

/// Cancel and delete any reviews (auto or manual) created at or after a commit's
/// `created_at` timestamp.  If a review has an active session, the session is
/// cancelled via the registry and then deleted.
pub(crate) fn cleanup_reviews_after_commit(
    store: &Arc<Store>,
    registry: &session_runner::SessionRegistry,
    commit: &crate::store::models::Commit,
) {
    // Only clean up reviews for commits that actually landed (have a SHA).
    // Pending/failed commits without a SHA never produced a reviewable diff.
    if commit.sha.is_none() {
        return;
    }

    let reviews = match store.find_reviews_created_since(&commit.branch_id, commit.created_at) {
        Ok(r) => r,
        Err(_) => return,
    };
    for review in reviews {
        if let Some(ref sid) = review.session_id {
            registry.cancel(sid);
            let _ = store.delete_session(sid);
        }
        let _ = store.delete_review(&review.id);
    }
}

/// Delete a review and all its comments, optionally deleting its linked session.
#[tauri::command(rename_all = "camelCase")]
pub fn delete_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;
    let review = store
        .get_review(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Review not found: {review_id}"))?;

    store.delete_review(&review_id).map_err(|e| e.to_string())?;

    if delete_session.unwrap_or(false) {
        if let Some(sid) = review.session_id {
            let _ = store.delete_session(&sid);
        }
    }
    Ok(())
}

/// Delete a pending commit (one with no SHA) by its DB id.
/// This does NOT touch git — it only removes the DB record and optionally its session.
#[tauri::command(rename_all = "camelCase")]
pub fn delete_pending_commit(
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    commit_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;

    let commit = store
        .get_commit(&commit_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Commit not found: {commit_id}"))?;

    // Safety check: only allow deleting commits that have no SHA (pending/failed)
    if commit.sha.is_some() {
        return Err(
            "Cannot use delete_pending_commit for commits with a SHA. Use delete_commit instead."
                .to_string(),
        );
    }

    // Clean up reviews created at or after this commit
    cleanup_reviews_after_commit(&store, &registry, &commit);

    store.delete_commit(&commit_id).map_err(|e| e.to_string())?;

    if delete_session.unwrap_or(false) {
        if let Some(sid) = commit.session_id {
            let _ = store.delete_session(&sid);
        }
    }

    Ok(())
}

/// Delete a commit: resets the branch HEAD to the parent commit,
/// removing the git commit, then cleans up the DB record and session.
///
/// Only works for the tip commit (HEAD) of the branch's worktree.
/// Returns an error if the commit is not the current HEAD.
#[tauri::command(rename_all = "camelCase")]
pub async fn delete_commit(
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;
    let registry = Arc::clone(&registry);

    tauri::async_runtime::spawn_blocking(move || {
        let branch = store
            .get_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

        if let Some(ref ws_name) = branch.workspace_name {
            // Remote branch: use blox::ws_exec with a single atomic shell command
            let repo_subpath = branches::resolve_branch_workspace_subpath(&store, &branch)
                .map_err(|e| e.to_string())?;
            let resolved_path = match repo_subpath.as_deref() {
                Some(subpath) => branches::resolve_workspace_repo_path(ws_name, subpath)
                    .map_err(|e| e.to_string())?,
                None => ".".to_string(),
            };

            // Single round-trip script: verify HEAD, get parent, reset.
            // Passing commit_sha as an argument avoids TOCTOU races.
            let script = concat!(
                "cd \"$1\" && ",
                "head=$(git rev-parse HEAD) && ",
                "case \"$head\" in \"$2\"*) ;; *) case \"$2\" in \"$head\"*) ;; *) ",
                "echo \"NOT_HEAD:$head\" >&2; exit 1 ;; esac ;; esac && ",
                "if ! git rev-parse \"$2^\" >/dev/null 2>&1; then ",
                "echo \"INITIAL_COMMIT\" >&2; exit 1; fi && ",
                "parent=$(git rev-parse \"$2^\") && ",
                "git reset --hard \"$parent\""
            );

            blox::ws_exec(
                ws_name,
                &["sh", "-c", script, "_", &resolved_path, &commit_sha],
            )
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("NOT_HEAD:") {
                    // Extract the actual HEAD sha from the error
                    if let Some(head) = msg.split("NOT_HEAD:").nth(1) {
                        let head = head
                            .trim()
                            .trim_end_matches(|c: char| !c.is_ascii_hexdigit());
                        let short_commit = &commit_sha[..7.min(commit_sha.len())];
                        let short_head = &head[..7.min(head.len())];
                        format!(
                            "Can only delete the latest commit. {} is not HEAD ({})",
                            short_commit, short_head
                        )
                    } else {
                        format!("Remote delete failed: {e}")
                    }
                } else if msg.contains("INITIAL_COMMIT") {
                    "Cannot delete the initial commit".to_string()
                } else {
                    format!("Remote delete failed: {e}")
                }
            })?;
        } else {
            // Local branch: use local worktree
            let workdir = store
                .get_workdir_for_branch(&branch_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

            let worktree = Path::new(&workdir.path);

            // Verify the commit is the current HEAD
            let head_sha = git::get_head_sha(worktree).map_err(|e| e.to_string())?;
            if !head_sha.starts_with(&commit_sha)
                && !commit_sha.starts_with(&head_sha)
                && head_sha != commit_sha
            {
                return Err(format!(
                    "Can only delete the latest commit. {} is not HEAD ({})",
                    &commit_sha[..7.min(commit_sha.len())],
                    &head_sha[..7.min(head_sha.len())]
                ));
            }

            // Find the parent commit
            let parent = git::get_parent_commit(worktree, &commit_sha)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Cannot delete the initial commit".to_string())?;

            // Reset to parent — this removes the commit from the branch
            git::reset_to_commit(worktree, &parent).map_err(|e| e.to_string())?;
        }

        // Clean up DB record if one exists
        if let Ok(Some(db_commit)) = store.get_commit_by_sha(&branch_id, &commit_sha) {
            // Clean up reviews created at or after this commit
            cleanup_reviews_after_commit(&store, &registry, &db_commit);

            let _ = store.delete_commit(&db_commit.id);

            if delete_session.unwrap_or(false) {
                if let Some(sid) = db_commit.session_id {
                    let _ = store.delete_session(&sid);
                }
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| format!("Delete task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Span;
    use crate::store::models::{Branch, Comment, Project, ReviewScope, Workdir};
    use crate::test_utils::TempGitRepo;

    fn repo_with_visible_and_stale_commit() -> (TempGitRepo, String, String) {
        let repo = TempGitRepo::new();
        repo.write_file("file.txt", "base\n");
        repo.commit("base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", "main"]);
        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("file.txt", "base\nvisible\n");
        let visible_sha = repo.commit("visible change");
        repo.write_file("file.txt", "base\nvisible\nstale\n");
        let stale_sha = repo.commit("stale change");
        repo.run_git(&["reset", "--hard", &visible_sha]);

        (repo, visible_sha, stale_sha)
    }

    fn store_with_branch(repo: &TempGitRepo) -> (Arc<Store>, Branch) {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let workdir =
            Workdir::new(&project.id, repo.path().to_str().unwrap()).with_branch(&branch.id);
        store.create_workdir(&workdir).unwrap();

        (store, branch)
    }

    #[test]
    fn build_branch_timeline_hides_reviews_for_commits_missing_from_git_history() {
        let (repo, visible_sha, stale_sha) = repo_with_visible_and_stale_commit();
        let (store, branch) = store_with_branch(&repo);

        let visible_review = Review::new(&branch.id, &visible_sha, ReviewScope::Commit);
        let stale_review = Review::new(&branch.id, &stale_sha, ReviewScope::Commit);
        store.create_review(&visible_review).unwrap();
        store.create_review(&stale_review).unwrap();

        let timeline = build_branch_timeline(&store, &branch.id).unwrap();

        assert_eq!(timeline.commits.len(), 1);
        assert_eq!(timeline.commits[0].sha, visible_sha);
        assert_eq!(timeline.reviews.len(), 1);
        assert_eq!(timeline.reviews[0].id, visible_review.id);
    }

    #[test]
    fn build_branch_timeline_hides_stale_reviews_with_only_agent_comments() {
        let (repo, _visible_sha, stale_sha) = repo_with_visible_and_stale_commit();
        let (store, branch) = store_with_branch(&repo);

        let stale_review = Review::new(&branch.id, &stale_sha, ReviewScope::Commit);
        let agent_comment = Comment::new("file.txt", Span::new(2, 3), "Agent note")
            .with_author(CommentAuthor::Agent);
        store.create_review(&stale_review).unwrap();
        store.add_comment(&stale_review.id, &agent_comment).unwrap();

        let timeline = build_branch_timeline(&store, &branch.id).unwrap();

        assert_eq!(timeline.commits.len(), 1);
        assert!(timeline.reviews.is_empty());
    }

    #[test]
    fn build_branch_timeline_keeps_stale_reviews_with_user_comments() {
        let (repo, _visible_sha, stale_sha) = repo_with_visible_and_stale_commit();
        let (store, branch) = store_with_branch(&repo);

        let stale_review = Review::new(&branch.id, &stale_sha, ReviewScope::Commit);
        let user_comment = Comment::new("file.txt", Span::new(2, 3), "Please follow up");
        store.create_review(&stale_review).unwrap();
        store.add_comment(&stale_review.id, &user_comment).unwrap();

        let timeline = build_branch_timeline(&store, &branch.id).unwrap();

        assert_eq!(timeline.commits.len(), 1);
        assert_eq!(timeline.reviews.len(), 1);
        assert_eq!(timeline.reviews[0].id, stale_review.id);
        assert_eq!(timeline.reviews[0].comment_count, 1);
    }

    #[test]
    fn build_branch_timeline_hides_stale_reviews_with_deleted_user_comments() {
        let (repo, _visible_sha, stale_sha) = repo_with_visible_and_stale_commit();
        let (store, branch) = store_with_branch(&repo);

        let stale_review = Review::new(&branch.id, &stale_sha, ReviewScope::Commit);
        let user_comment = Comment::new("file.txt", Span::new(2, 3), "Please follow up");
        store.create_review(&stale_review).unwrap();
        store.add_comment(&stale_review.id, &user_comment).unwrap();
        store.delete_comment(&user_comment.id).unwrap();

        let timeline = build_branch_timeline(&store, &branch.id).unwrap();

        assert_eq!(timeline.commits.len(), 1);
        assert!(timeline.reviews.is_empty());
    }
}
