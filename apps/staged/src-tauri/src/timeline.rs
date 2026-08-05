//! Timeline — branch timeline construction and related commands.
//!
//! `get_branch_timeline` always uses `FetchMode::Never` so it returns
//! instantly from locally-cached refs. Git state rows show stale-but-present
//! data until refreshed.
//!
//! `refresh_branch_git_state` runs a TTL-gated `git fetch` + ref comparison
//! and emits a `git-state-updated` event that the frontend merges into the
//! existing timeline.

use crate::git;
use crate::session_runner;
use crate::store::{CommentAuthor, ResolvedSession, Review, Store};
use crate::{
    blox, branches, BranchTimeline, CommitTimelineItem, ImageTimelineItem, NoteTimelineItem,
    ReviewTimelineItem,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Emitter;

/// TTL for cached git user identity lookups (5 minutes).
const GIT_USER_IDENTITY_TTL_MS: u128 = 300_000;

#[derive(Debug, Clone)]
struct GitUserIdentity {
    name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Clone)]
struct GitUserIdentityCacheEntry {
    identity: GitUserIdentity,
    fetched_at: u128,
}

fn git_user_identity_cache() -> &'static Mutex<HashMap<String, GitUserIdentityCacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, GitUserIdentityCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_git_user_identity<F>(cache_key: &str, fetch: F) -> GitUserIdentity
where
    F: FnOnce() -> GitUserIdentity,
{
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    if let Ok(cache) = git_user_identity_cache().lock() {
        if let Some(entry) = cache.get(cache_key) {
            if now.saturating_sub(entry.fetched_at) < GIT_USER_IDENTITY_TTL_MS {
                return entry.identity.clone();
            }
        }
    }

    let identity = fetch();

    if let Ok(mut cache) = git_user_identity_cache().lock() {
        cache.insert(
            cache_key.to_string(),
            GitUserIdentityCacheEntry {
                identity: identity.clone(),
                fetched_at: now,
            },
        );
    }

    identity
}

/// Extract the GitHub username from a noreply email address.
/// Matches both `<username>@users.noreply.github.com` and
/// `<id>+<username>@users.noreply.github.com`.
fn github_noreply_username(email: &str) -> Option<&str> {
    let local = email.strip_suffix("@users.noreply.github.com")?;
    // `<id>+<username>` form
    if let Some((_id, username)) = local.split_once('+') {
        Some(username)
    } else {
        Some(local)
    }
}

/// Check whether a commit was authored by the configured git user.
///
/// Matches on name OR email (case-insensitive). Also handles GitHub
/// noreply emails by comparing the embedded username against the local
/// user name.
fn is_commit_by_user(commit_author: &str, commit_email: &str, identity: &GitUserIdentity) -> bool {
    let has_identity = identity.name.is_some() || identity.email.is_some();
    if !has_identity {
        // When we don't know who the user is, we can't claim ownership.
        return false;
    }

    // Case-insensitive name match
    if let Some(ref name) = identity.name {
        if !commit_author.is_empty() && commit_author.eq_ignore_ascii_case(name) {
            return true;
        }
    }

    // Case-insensitive email match
    if let Some(ref email) = identity.email {
        if !commit_email.is_empty() && commit_email.eq_ignore_ascii_case(email) {
            return true;
        }
    }

    // GitHub noreply heuristic: if the commit email is a noreply, compare
    // the embedded username against the configured user name.
    if let Some(gh_username) = github_noreply_username(commit_email) {
        if let Some(ref name) = identity.name {
            if gh_username.eq_ignore_ascii_case(name) {
                return true;
            }
        }
    }

    // Reverse: if the local email is a noreply, compare its username
    // against the commit author name.
    if let Some(ref email) = identity.email {
        if let Some(gh_username) = github_noreply_username(email) {
            if !commit_author.is_empty() && commit_author.eq_ignore_ascii_case(gh_username) {
                return true;
            }
        }
    }

    false
}

/// Payload for the `git-state-updated` event emitted by `refresh_branch_git_state`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitStateUpdatedPayload {
    branch_id: String,
    git_state: git::BranchGitState,
}

/// A single commit on the parent branch that hasn't yet been merged into the
/// current branch — returned by `list_parent_branch_commits` for the hover
/// popover on the parent-branch capsule.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentBranchCommit {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub author_email: String,
    pub timestamp: i64,
}

fn parse_parent_commit_lines(lines: &[String]) -> Vec<ParentBranchCommit> {
    lines
        .iter()
        .filter_map(|line| git::parse_branch_commit_line(line))
        .map(|fields| ParentBranchCommit {
            sha: fields.sha.to_string(),
            short_sha: fields.short_sha.to_string(),
            subject: fields.subject.to_string(),
            author: fields.author.to_string(),
            author_email: fields.author_email.to_string(),
            // Committer time: these commits are listed on their own, never
            // interleaved with notes, so there's nothing for a rebase-stable
            // clock to line up with.
            timestamp: fields.committer_timestamp,
        })
        .collect()
}

/// Parse [`git::BRANCH_COMMIT_LOG_FORMAT`] commit lines into timeline items,
/// looking up DB metadata for session linkage.
fn parse_commit_lines(
    store: &Arc<Store>,
    branch_id: &str,
    lines: &[String],
) -> Vec<CommitTimelineItem> {
    let mut commits = Vec::new();
    for line in lines {
        if let Some(fields) = git::parse_branch_commit_line(line) {
            let sha = fields.sha.to_string();
            let our_commit = store.get_commit_by_sha(branch_id, &sha).unwrap_or(None);
            let resolved = store
                .resolve_session_status(our_commit.as_ref().and_then(|c| c.session_id.as_deref()));
            commits.push(CommitTimelineItem {
                id: our_commit.as_ref().map(|c| c.id.clone()),
                sha,
                short_sha: fields.short_sha.to_string(),
                subject: fields.subject.to_string(),
                author: fields.author.to_string(),
                author_email: fields.author_email.to_string(),
                timestamp: fields.author_timestamp,
                sort_timestamp: fields.author_timestamp,
                order: 0,
                session_id: resolved.session_id,
                session_status: resolved.status,
                completion_reason: resolved.completion_reason,
                is_own_commit: false, // set later by build_branch_timeline
            });
        }
    }
    let len = commits.len() as i64;
    for (i, commit) in commits.iter_mut().enumerate() {
        commit.order = len - 1 - i as i64;
    }
    commits
}

/// Fetch commits from a remote workspace using merge-base + git log.
fn fetch_remote_commits(
    ws_name: &str,
    repo_subpath: Option<&str>,
    store: &Arc<Store>,
    branch_id: &str,
    base_ref: &str,
) -> Result<Vec<CommitTimelineItem>, String> {
    let range = if let Ok(mb_output) =
        branches::run_workspace_git(ws_name, repo_subpath, &["merge-base", base_ref, "HEAD"])
    {
        let mb = mb_output.trim().to_string();
        format!("{mb}..HEAD")
    } else {
        format!("{base_ref}..HEAD")
    };
    let output = branches::run_workspace_git(
        ws_name,
        repo_subpath,
        &["log", git::BRANCH_COMMIT_LOG_FORMAT, &range],
    )
    .map_err(|e| format!("Failed to load commits from workspace: {e}"))?;
    let lines: Vec<String> = output
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(parse_commit_lines(store, branch_id, &lines))
}

/// Map local CommitInfo entries to CommitTimelineItems with DB metadata.
fn map_local_commits(
    store: &Arc<Store>,
    branch_id: &str,
    git_commits: &[git::CommitInfo],
) -> Vec<CommitTimelineItem> {
    git_commits
        .iter()
        .map(|gc| {
            let our_commit = store.get_commit_by_sha(branch_id, &gc.sha).unwrap_or(None);
            let resolved = store
                .resolve_session_status(our_commit.as_ref().and_then(|c| c.session_id.as_deref()));
            CommitTimelineItem {
                id: our_commit.as_ref().map(|c| c.id.clone()),
                sha: gc.sha.clone(),
                short_sha: gc.short_sha.clone(),
                subject: gc.subject.clone(),
                author: gc.author.clone(),
                author_email: gc.author_email.clone(),
                timestamp: gc.author_timestamp,
                sort_timestamp: gc.author_timestamp,
                order: gc.order,
                session_id: resolved.session_id,
                session_status: resolved.status,
                completion_reason: resolved.completion_reason,
                is_own_commit: false, // set later by build_branch_timeline
            }
        })
        .collect()
}

/// The agent-pushed ACP session title, when set and non-empty.
///
/// Applied at read time only, as an interim display name for in-progress
/// timeline rows — stored artifact names (commit subject, note H1, review
/// title) always win once the session ends.
fn non_empty_acp_title(resolved: &ResolvedSession) -> Option<String> {
    resolved
        .acp_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Clamp commit sort keys so they never decrease in branch order.
///
/// Committer dates are naturally non-decreasing along a branch; the author
/// dates the timeline sorts on aren't — a cherry-pick keeps its original
/// author date, and an interactive rebase can reorder commits — so a commit
/// could otherwise sort above one that precedes it in `git log`. Walking
/// oldest-first with a running max pins each commit to at least its
/// predecessor's effective time, keeping the rendered order the same as git's.
///
/// Only `sort_timestamp` moves; `timestamp` keeps the real author date, which
/// is what the UI renders.
///
/// `commits` arrives in `git log` order (newest first), as both producers emit
/// it, so the walk runs in reverse.
fn clamp_commit_sort_timestamps(commits: &mut [CommitTimelineItem]) {
    clamp_timestamps_monotonic(commits.iter_mut().rev().map(|c| &mut c.sort_timestamp));
}

/// Raise each timestamp to at least its predecessor's, in iteration order.
///
/// Shared by the two timelines that interleave git commits with DB-timed items
/// and so have to sort on author date: this module's branch timeline and the
/// agent-facing branch history in `session_commands`. Callers pass their
/// timestamps in branch order, oldest first.
pub(crate) fn clamp_timestamps_monotonic<'a>(timestamps: impl Iterator<Item = &'a mut i64>) {
    let mut floor = i64::MIN;
    for timestamp in timestamps {
        floor = (*timestamp).max(floor);
        *timestamp = floor;
    }
}

/// Public wrapper for `build_branch_timeline` for use by the web server.
pub fn build_branch_timeline_public(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<BranchTimeline, String> {
    build_branch_timeline(store, branch_id)
}

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
    let mut identity = GitUserIdentity {
        name: None,
        email: None,
    };

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
        let resolved_path = resolve_repo_path(ws_name, repo_subpath.as_deref())?;
        let base_ref = git::origin_ref_for_branch(&branch.base_branch);

        // Foreground: skip untracked enumeration so first paint isn't blocked
        // by a recursive working-tree walk on huge monorepos. The background
        // refresh re-runs with `Full` and emits the corrected counts via
        // `git-state-updated`.
        git_state = Some(git::compute_branch_git_state_batched(
            &cache_key,
            |script, args| {
                branches::run_workspace_shell(ws_name, script, args).map_err(|e| e.to_string())
            },
            &resolved_path,
            &branch.branch_name,
            &branch.base_branch,
            git::FetchMode::Never,
            git::WorktreeStatusScope::Indexed,
        ));

        {
            let ws = ws_name.clone();
            let sub = repo_subpath.clone();
            identity = cached_git_user_identity(
                &format!("remote:{ws}:{}", sub.as_deref().unwrap_or("")),
                || {
                    let name =
                        branches::run_workspace_git(&ws, sub.as_deref(), &["config", "user.name"])
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                    let email =
                        branches::run_workspace_git(&ws, sub.as_deref(), &["config", "user.email"])
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                    GitUserIdentity { name, email }
                },
            );
        }

        commits = fetch_remote_commits(
            ws_name,
            repo_subpath.as_deref(),
            store,
            branch_id,
            &base_ref,
        )?;
    } else if let Some(ref wd) = workdir {
        // Local branch: fetch commits from the local worktree
        let worktree_path = Path::new(&wd.path);
        if worktree_path.exists() {
            let base_ref = git::origin_ref_for_branch(&branch.base_branch);

            // Foreground: `Indexed` skips the untracked-file walk and lite-env
            // skips the per-project `$SHELL -ils` capture. A captured-env
            // warm-up still fires in the background so the next non-foreground
            // op (refresh, pull, discard) finds a ready snapshot.
            git_state = Some(git::compute_local_branch_git_state(
                worktree_path,
                &branch.branch_name,
                &branch.base_branch,
                git::FetchMode::Never,
                git::WorktreeStatusScope::Indexed,
                git::EnvSource::Lite,
            ));

            {
                let path_str = wd.path.clone();
                // `git config user.{name,email}` reads `.git/config` + `~/.gitconfig`
                // — env-independent — so the lite path is enough and we don't
                // want to block first paint on the per-project `$SHELL -ils`
                // capture (~8.5s on cold cache).
                identity = cached_git_user_identity(&format!("local:{path_str}"), || {
                    let name = git::cli_run_smart(worktree_path, &["config", "user.name"])
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    let email = git::cli_run_smart(worktree_path, &["config", "user.email"])
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    GitUserIdentity { name, email }
                });
            }

            let git_commits =
                git::get_commits_since_base(worktree_path, &base_ref).map_err(|e| {
                    format!("Failed to get commits since base for branch {branch_id}: {e:?}")
                })?;
            commits = map_local_commits(store, branch_id, &git_commits);
        }
    }

    clamp_commit_sort_timestamps(&mut commits);

    // Mark commits authored by the current user
    for commit in &mut commits {
        if !commit.sha.is_empty() {
            commit.is_own_commit =
                is_commit_by_user(&commit.author, &commit.author_email, &identity);
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
                subject: non_empty_acp_title(&resolved)
                    .or_else(|| {
                        resolved.session_id.as_deref().and_then(|sid| {
                            store
                                .get_session(sid)
                                .ok()
                                .flatten()
                                .map(|s| s.prompt.clone())
                        })
                    })
                    .unwrap_or_else(|| "Pending commit".to_string()),
                author: String::new(),
                author_email: String::new(),
                timestamp: dc.created_at / 1000,
                sort_timestamp: dc.created_at / 1000,
                order: 0,
                session_id: resolved.session_id,
                session_status: resolved.status,
                completion_reason: resolved.completion_reason,
                is_own_commit: true, // pending commits are always the current user's
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
            // While the session runs, the stored title is just the prompt
            // stub — an agent-pushed ACP title is a better interim name.
            // Once the session ends the stored title is final (note H1 or
            // prompt fallback) and always wins.
            let title = if resolved.status.as_deref() == Some("running") {
                non_empty_acp_title(&resolved).unwrap_or(n.title)
            } else {
                n.title
            };
            NoteTimelineItem {
                id: n.id,
                title,
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
        .filter(|r| review_is_visible_in_timeline(r, |sha| visible_shas.contains(sha)))
        .map(|r| {
            let resolved = store.resolve_session_status(r.session_id.as_deref());
            let comment_count = r.comments.len();
            // Untitled running reviews would otherwise render a static
            // placeholder; surface the agent-pushed ACP title instead. A
            // stored title always wins.
            let title = match r.title {
                None if resolved.status.as_deref() == Some("running") => {
                    non_empty_acp_title(&resolved)
                }
                title => title,
            };
            ReviewTimelineItem {
                id: r.id,
                commit_sha: r.commit_sha,
                scope: r.scope.as_str().to_string(),
                session_id: resolved.session_id,
                session_status: resolved.status,
                session_provider: resolved.provider,
                completion_reason: resolved.completion_reason,
                title,
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

fn resolve_repo_path(ws_name: &str, repo_subpath: Option<&str>) -> Result<String, String> {
    match repo_subpath.map(str::trim).filter(|s| !s.is_empty()) {
        Some(subpath) => {
            branches::resolve_workspace_repo_path(ws_name, subpath).map_err(|e| e.to_string())
        }
        None => Ok(".".to_string()),
    }
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

/// Whether a review should be shown in a branch's timeline.
///
/// A review is hidden once its originating commit is no longer on the branch
/// (e.g. rebased or squashed away), unless the user has commented on it. This
/// rule is shared between the branch card timeline and the session-context
/// branch history (see `session_commands::review_timeline_entries`) so the two
/// can't drift apart.
pub(crate) fn review_is_visible_in_timeline(
    review: &Review,
    sha_is_visible: impl Fn(&str) -> bool,
) -> bool {
    review.commit_sha.is_empty()
        || sha_is_visible(review.commit_sha.as_str())
        || review
            .comments
            .iter()
            .any(|comment| comment.author == CommentAuthor::User)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_branch_timeline(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<BranchTimeline, String> {
    let store = crate::get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || build_branch_timeline(&store, &branch_id))
        .await
        .map_err(|e| format!("Timeline task failed: {e}"))?
}

/// Run a `git fetch` + git state recomputation for a branch, then emit
/// a `git-state-updated` event so the frontend can merge the fresh state
/// into the existing timeline. Defaults to TTL-gated fetching; pass
/// `force = true` to bypass the TTL (e.g. right after a successful push,
/// where the caller knows the remote has moved).
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_branch_git_state(
    app: tauri::AppHandle,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    force: Option<bool>,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        refresh_branch_git_state_impl(&app, &store, &branch_id, force)
    })
    .await
    .map_err(|e| format!("Git state refresh task failed: {e}"))?
}

/// Synchronous body of [`refresh_branch_git_state`], shared verbatim with the
/// web-mode `dispatch()` arm. Callers run it inside `spawn_blocking`.
pub(crate) fn refresh_branch_git_state_impl(
    app: &tauri::AppHandle,
    store: &Arc<Store>,
    branch_id: &str,
    force: Option<bool>,
) -> Result<(), String> {
    let fetch_mode = if force.unwrap_or(false) {
        git::FetchMode::Force
    } else {
        git::FetchMode::Ttl
    };

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?;

    let git_state = if let Some(ref ws_name) = branch.workspace_name {
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        let cache_key = remote_git_state_cache_key(
            ws_name,
            repo_subpath.as_deref(),
            &branch.branch_name,
            &branch.base_branch,
        );
        let resolved_path = resolve_repo_path(ws_name, repo_subpath.as_deref())?;

        Some(git::compute_branch_git_state_batched(
            &cache_key,
            |script, args| {
                branches::run_workspace_shell(ws_name, script, args).map_err(|e| e.to_string())
            },
            &resolved_path,
            &branch.branch_name,
            &branch.base_branch,
            fetch_mode,
            git::WorktreeStatusScope::Full,
        ))
    } else if let Some(ref wd) = workdir {
        let worktree_path = Path::new(&wd.path);
        if worktree_path.exists() {
            Some(git::compute_local_branch_git_state(
                worktree_path,
                &branch.branch_name,
                &branch.base_branch,
                fetch_mode,
                git::WorktreeStatusScope::Full,
                git::EnvSource::Captured,
            ))
        } else {
            None
        }
    } else {
        None
    };

    if let Some(state) = git_state {
        let _ = app.emit(
            "git-state-updated",
            GitStateUpdatedPayload {
                branch_id: branch_id.to_string(),
                git_state: state,
            },
        );
    }
    Ok(())
}

/// Return the commits on the parent branch that aren't yet in this branch's
/// ancestry — i.e. the same `merge-base(HEAD, origin/{base})..origin/{base}`
/// range that `commitsSinceFork` counts. Read-only against locally cached
/// refs; the count's own fetch round-trip is what keeps `origin/{base}`
/// current.
#[tauri::command(rename_all = "camelCase")]
pub async fn list_parent_branch_commits(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<Vec<ParentBranchCommit>, String> {
    let store = crate::get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || {
        list_parent_branch_commits_impl(&store, &branch_id)
    })
    .await
    .map_err(|e| format!("List parent branch commits task failed: {e}"))?
}

/// Synchronous body of [`list_parent_branch_commits`], shared verbatim with the
/// web-mode `dispatch()` arm. Callers run it inside `spawn_blocking`.
pub(crate) fn list_parent_branch_commits_impl(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<Vec<ParentBranchCommit>, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let base_ref = git::origin_ref_for_branch(&branch.base_branch);
    let format_arg = git::BRANCH_COMMIT_LOG_FORMAT;

    if let Some(ref ws_name) = branch.workspace_name {
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        let range = match branches::run_workspace_git(
            ws_name,
            repo_subpath.as_deref(),
            &["merge-base", &base_ref, "HEAD"],
        ) {
            Ok(mb_output) => format!("{}..{base_ref}", mb_output.trim()),
            Err(_) => base_ref.clone(),
        };
        let output = match branches::run_workspace_git(
            ws_name,
            repo_subpath.as_deref(),
            &["log", "--max-count=26", format_arg, &range],
        ) {
            Ok(o) => o,
            Err(_) => return Ok(Vec::new()),
        };
        let lines: Vec<String> = output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();
        return Ok(parse_parent_commit_lines(&lines));
    }

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    let Some(wd) = workdir else {
        return Ok(Vec::new());
    };
    let worktree_path = Path::new(&wd.path);
    if !worktree_path.exists() {
        return Ok(Vec::new());
    }

    let range = match git::cli_run_smart(worktree_path, &["merge-base", &base_ref, "HEAD"]) {
        Ok(mb_output) => format!("{}..{base_ref}", mb_output.trim()),
        Err(_) => base_ref.clone(),
    };
    let output = match git::cli_run_smart(
        worktree_path,
        &["log", "--max-count=26", format_arg, &range],
    ) {
        Ok(o) => o,
        Err(_) => return Ok(Vec::new()),
    };
    let lines: Vec<String> = output
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(parse_parent_commit_lines(&lines))
}

/// Fast-forward the branch to its upstream, right now.
///
/// The immediate half of `prs::pull_or_queue_branch`, which owns the decision
/// between pulling now and queueing behind in-flight branch sessions. Callers run
/// this inside `spawn_blocking`.
pub(crate) fn pull_branch_ff_only_impl(store: &Arc<Store>, branch_id: &str) -> Result<(), String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if let Some(ref ws_name) = branch.workspace_name {
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        let resolved_path = resolve_repo_path(ws_name, repo_subpath.as_deref())?;
        let cache_key = remote_git_state_cache_key(
            ws_name,
            repo_subpath.as_deref(),
            &branch.branch_name,
            &branch.base_branch,
        );
        let state = git::compute_branch_git_state_batched(
            &cache_key,
            |script, args| {
                branches::run_workspace_shell(ws_name, script, args).map_err(|e| e.to_string())
            },
            &resolved_path,
            &branch.branch_name,
            &branch.base_branch,
            git::FetchMode::Force,
            git::WorktreeStatusScope::Full,
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
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;
    let worktree = Path::new(&workdir.path);
    let state = git::compute_local_branch_git_state(
        worktree,
        &branch.branch_name,
        &branch.base_branch,
        git::FetchMode::Force,
        git::WorktreeStatusScope::Full,
        git::EnvSource::Captured,
    );
    git::ensure_fast_forward_pullable(&state)?;
    git::fast_forward_to_ref(worktree, &state.upstream.r#ref).map_err(|e| e.to_string())?;
    Ok(())
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

fn ensure_reset_to_remote_allowed(state: &git::BranchGitState) -> Result<(), String> {
    if state.detached_head {
        return Err("Cannot reset while HEAD is detached".to_string());
    }
    if !state.expected_branch_matches {
        let current = state
            .current_branch
            .as_deref()
            .unwrap_or("an unknown branch");
        return Err(format!("Cannot reset while checked out on {current}"));
    }
    if state.fetch.status == git::FetchStatus::Failed {
        let detail = state
            .fetch
            .error
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("latest origin state could not be fetched");
        return Err(format!("Cannot reset to origin: {detail}"));
    }
    if !state.upstream.exists {
        return Err(format!(
            "Cannot reset because {} does not exist",
            state.upstream.r#ref
        ));
    }
    if state.upstream.relation != git::UpstreamRelation::Diverged {
        return Err(
            "Reset to Origin is only available when the branch has diverged from origin"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn reset_branch_to_remote_impl(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<(), String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if let Some(ref ws_name) = branch.workspace_name {
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        let resolved_path = resolve_repo_path(ws_name, repo_subpath.as_deref())?;
        let cache_key = remote_git_state_cache_key(
            ws_name,
            repo_subpath.as_deref(),
            &branch.branch_name,
            &branch.base_branch,
        );
        let state = git::compute_branch_git_state_batched(
            &cache_key,
            |script, args| {
                branches::run_workspace_shell(ws_name, script, args).map_err(|e| e.to_string())
            },
            &resolved_path,
            &branch.branch_name,
            &branch.base_branch,
            git::FetchMode::Force,
            git::WorktreeStatusScope::Full,
        );
        ensure_reset_to_remote_allowed(&state)?;
        branches::run_workspace_git(
            ws_name,
            repo_subpath.as_deref(),
            &["reset", "--hard", &state.upstream.r#ref],
        )
        .map_err(|e| e.to_string())?;
        branches::run_workspace_git(ws_name, repo_subpath.as_deref(), &["clean", "-fd"])
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;
    let worktree = Path::new(&workdir.path);
    let state = git::compute_local_branch_git_state(
        worktree,
        &branch.branch_name,
        &branch.base_branch,
        git::FetchMode::Force,
        git::WorktreeStatusScope::Full,
        git::EnvSource::Captured,
    );
    ensure_reset_to_remote_allowed(&state)?;
    git::cli_run(worktree, &["reset", "--hard", &state.upstream.r#ref])
        .map_err(|e| e.to_string())?;
    git::cli_run(worktree, &["clean", "-fd"]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reset_branch_to_remote(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || reset_branch_to_remote_impl(&store, &branch_id))
        .await
        .map_err(|e| format!("Reset task failed: {e}"))?
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
        get_worktree_changes_preview_impl(&store, &branch_id)
    })
    .await
    .map_err(|e| format!("Worktree preview task failed: {e}"))?
}

/// Synchronous body of [`get_worktree_changes_preview`], shared verbatim with
/// the web-mode `dispatch()` arm. Callers run it inside `spawn_blocking`.
pub(crate) fn get_worktree_changes_preview_impl(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<WorktreeChangesPreview, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if let Some(ref ws_name) = branch.workspace_name {
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        return remote_worktree_change_paths(ws_name, repo_subpath.as_deref())
            .map(preview_from_change_paths);
    }

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;
    let paths =
        git::list_worktree_change_paths(Path::new(&workdir.path)).map_err(|e| e.to_string())?;
    Ok(preview_from_change_paths(paths))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn discard_worktree_changes(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    expected_preview: Option<WorktreeChangesPreview>,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || {
        discard_worktree_changes_impl(&store, &branch_id, expected_preview)
    })
    .await
    .map_err(|e| format!("Discard task failed: {e}"))?
}

/// Synchronous body of [`discard_worktree_changes`], shared verbatim with the
/// web-mode `dispatch()` arm. Callers run it inside `spawn_blocking`.
pub(crate) fn discard_worktree_changes_impl(
    store: &Arc<Store>,
    branch_id: &str,
    expected_preview: Option<WorktreeChangesPreview>,
) -> Result<(), String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if let Some(ref ws_name) = branch.workspace_name {
        let repo_subpath = branches::resolve_branch_workspace_subpath(store, &branch)?;
        let resolved_path = resolve_repo_path(ws_name, repo_subpath.as_deref())?;
        let cache_key = remote_git_state_cache_key(
            ws_name,
            repo_subpath.as_deref(),
            &branch.branch_name,
            &branch.base_branch,
        );
        let state = git::compute_branch_git_state_batched(
            &cache_key,
            |script, args| {
                branches::run_workspace_shell(ws_name, script, args).map_err(|e| e.to_string())
            },
            &resolved_path,
            &branch.branch_name,
            &branch.base_branch,
            git::FetchMode::Never,
            git::WorktreeStatusScope::Full,
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
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;
    let worktree = Path::new(&workdir.path);
    let state = git::compute_local_branch_git_state(
        worktree,
        &branch.branch_name,
        &branch.base_branch,
        git::FetchMode::Never,
        git::WorktreeStatusScope::Full,
        git::EnvSource::Captured,
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
    delete_commit_impl(registry, store, branch_id, commit_sha, delete_session).await
}

pub(crate) async fn delete_commit_impl(
    registry: Arc<session_runner::SessionRegistry>,
    store: Arc<Store>,
    branch_id: String,
    commit_sha: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
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
    use crate::store::models::{
        Branch, Comment, Commit, Note, Project, ReviewScope, Session, SessionStatus, Workdir,
    };
    use crate::test_utils::TempGitRepo;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use uuid::Uuid;

    struct TempPath {
        path: PathBuf,
    }

    impl TempPath {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("staged-{label}-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn run_git(repo: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        command
            .arg("-c")
            .arg("core.hooksPath=/dev/null")
            .arg("-C")
            .arg(repo)
            .args(args);
        crate::git::strip_git_env(&mut command);

        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn clone_repo(origin: &TempGitRepo) -> TempPath {
        let clone_dir = TempPath::new("clone");
        let mut command = Command::new("git");
        command.arg("clone").arg(origin.path()).arg(&clone_dir.path);
        crate::git::strip_git_env(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        run_git(
            &clone_dir.path,
            &["config", "user.email", "test@example.com"],
        );
        run_git(&clone_dir.path, &["config", "user.name", "Test"]);
        clone_dir
    }

    fn remote_backed_feature() -> (TempGitRepo, TempPath) {
        let origin = TempGitRepo::new();
        origin.write_file("file.txt", "base\n");
        origin.commit("base");

        let clone = clone_repo(&origin);
        run_git(&clone.path, &["checkout", "-b", "feature"]);
        fs::write(clone.path.join("file.txt"), "base\nfeature\n").unwrap();
        run_git(&clone.path, &["add", "."]);
        run_git(&clone.path, &["commit", "-m", "feature"]);
        run_git(&clone.path, &["push", "origin", "feature:feature"]);
        run_git(&clone.path, &["fetch", "origin", "main", "feature"]);
        (origin, clone)
    }

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

    fn store_with_branch_path(path: &Path) -> (Arc<Store>, Branch) {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let workdir = Workdir::new(&project.id, path.to_str().unwrap()).with_branch(&branch.id);
        store.create_workdir(&workdir).unwrap();

        (store, branch)
    }

    fn store_with_branch(repo: &TempGitRepo) -> (Arc<Store>, Branch) {
        store_with_branch_path(repo.path())
    }

    #[test]
    fn reset_branch_to_remote_resets_diverged_branch_and_cleans_worktree() {
        let (origin, clone) = remote_backed_feature();
        fs::write(clone.path.join("local.txt"), "local\n").unwrap();
        run_git(&clone.path, &["add", "."]);
        run_git(&clone.path, &["commit", "-m", "local"]);

        origin.run_git(&["checkout", "feature"]);
        origin.write_file("origin.txt", "origin\n");
        let origin_sha = origin.commit("origin");

        fs::write(clone.path.join("file.txt"), "dirty\n").unwrap();
        fs::write(clone.path.join("untracked.txt"), "untracked\n").unwrap();
        let (store, branch) = store_with_branch_path(&clone.path);

        reset_branch_to_remote_impl(&store, &branch.id).unwrap();

        let head = run_git(&clone.path, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let upstream = run_git(&clone.path, &["rev-parse", "origin/feature"])
            .trim()
            .to_string();
        assert_eq!(head, upstream);
        assert_eq!(head, origin_sha);
        assert!(!clone.path.join("local.txt").exists());
        assert!(!clone.path.join("untracked.txt").exists());
        assert!(clone.path.join("origin.txt").exists());
        assert_eq!(run_git(&clone.path, &["status", "--porcelain"]).trim(), "");
    }

    #[test]
    fn reset_branch_to_remote_rejects_local_ahead_branch() {
        let (_origin, clone) = remote_backed_feature();
        fs::write(clone.path.join("local.txt"), "local\n").unwrap();
        run_git(&clone.path, &["add", "."]);
        run_git(&clone.path, &["commit", "-m", "local"]);
        let local_sha = run_git(&clone.path, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let (store, branch) = store_with_branch_path(&clone.path);

        let err = reset_branch_to_remote_impl(&store, &branch.id).unwrap_err();

        assert!(err.contains("only available when the branch has diverged"));
        assert_eq!(
            run_git(&clone.path, &["rev-parse", "HEAD"])
                .trim()
                .to_string(),
            local_sha
        );
    }

    #[test]
    fn reset_branch_to_remote_rejects_missing_upstream() {
        let origin = TempGitRepo::new();
        origin.write_file("file.txt", "base\n");
        origin.commit("base");
        let clone = clone_repo(&origin);
        run_git(&clone.path, &["checkout", "-b", "feature"]);
        fs::write(clone.path.join("file.txt"), "base\nfeature\n").unwrap();
        run_git(&clone.path, &["add", "."]);
        run_git(&clone.path, &["commit", "-m", "feature"]);
        let (store, branch) = store_with_branch_path(&clone.path);

        let err = reset_branch_to_remote_impl(&store, &branch.id).unwrap_err();

        assert!(err.contains("origin/feature does not exist"));
    }

    #[test]
    fn reset_branch_to_remote_rejects_wrong_checked_out_branch() {
        let (_origin, clone) = remote_backed_feature();
        run_git(&clone.path, &["checkout", "-b", "other"]);
        let (store, branch) = store_with_branch_path(&clone.path);

        let err = reset_branch_to_remote_impl(&store, &branch.id).unwrap_err();

        assert!(err.contains("checked out on other"));
    }

    #[test]
    fn reset_branch_to_remote_rejects_detached_head() {
        let (_origin, clone) = remote_backed_feature();
        let head = run_git(&clone.path, &["rev-parse", "HEAD"]);
        run_git(&clone.path, &["checkout", "--detach", head.trim()]);
        let (store, branch) = store_with_branch_path(&clone.path);

        let err = reset_branch_to_remote_impl(&store, &branch.id).unwrap_err();

        assert!(err.contains("HEAD is detached"));
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

    fn running_session(store: &Arc<Store>, prompt: &str, acp_title: Option<&str>) -> Session {
        let session = Session::new_running(prompt, Path::new("/tmp"));
        store.create_session(&session).unwrap();
        if let Some(title) = acp_title {
            store
                .update_session_acp_title(&session.id, Some(title))
                .unwrap();
        }
        session
    }

    #[test]
    fn build_branch_timeline_pending_commit_prefers_acp_title_over_prompt() {
        let (repo, _visible_sha, _stale_sha) = repo_with_visible_and_stale_commit();
        let (store, branch) = store_with_branch(&repo);

        let titled = running_session(&store, "fix the login bug", Some("Fix login token refresh"));
        let untitled = running_session(&store, "add more tests", None);
        let titled_commit = Commit::new_pending(&branch.id).with_session(&titled.id);
        let untitled_commit = Commit::new_pending(&branch.id).with_session(&untitled.id);
        store.create_commit(&titled_commit).unwrap();
        store.create_commit(&untitled_commit).unwrap();

        let timeline = build_branch_timeline(&store, &branch.id).unwrap();

        let subject_of = |commit_id: &str| {
            timeline
                .commits
                .iter()
                .find(|c| c.id.as_deref() == Some(commit_id))
                .unwrap()
                .subject
                .clone()
        };
        assert_eq!(subject_of(&titled_commit.id), "Fix login token refresh");
        assert_eq!(subject_of(&untitled_commit.id), "add more tests");
    }

    #[test]
    fn build_branch_timeline_running_note_shows_acp_title() {
        let (repo, _visible_sha, _stale_sha) = repo_with_visible_and_stale_commit();
        let (store, branch) = store_with_branch(&repo);

        let titled = running_session(&store, "write up the plan", Some("Plan: staged rollout"));
        let untitled = running_session(&store, "summarize findings", None);
        let titled_note = Note::new(&branch.id, "write up the plan", "").with_session(&titled.id);
        let untitled_note =
            Note::new(&branch.id, "summarize findings", "").with_session(&untitled.id);
        store.create_note(&titled_note).unwrap();
        store.create_note(&untitled_note).unwrap();

        let timeline = build_branch_timeline(&store, &branch.id).unwrap();

        let title_of = |note_id: &str| {
            timeline
                .notes
                .iter()
                .find(|n| n.id == note_id)
                .unwrap()
                .title
                .clone()
        };
        assert_eq!(title_of(&titled_note.id), "Plan: staged rollout");
        assert_eq!(title_of(&untitled_note.id), "summarize findings");
    }

    #[test]
    fn build_branch_timeline_running_review_shows_acp_title() {
        let (repo, visible_sha, _stale_sha) = repo_with_visible_and_stale_commit();
        let (store, branch) = store_with_branch(&repo);

        let session = running_session(&store, "review the change", Some("Review: token refresh"));
        let untitled_review =
            Review::new(&branch.id, &visible_sha, ReviewScope::Commit).with_session(&session.id);
        store.create_review(&untitled_review).unwrap();

        let stored_session = running_session(&store, "another review", Some("Should not display"));
        let mut stored_review = Review::new(&branch.id, &visible_sha, ReviewScope::Commit)
            .with_session(&stored_session.id);
        stored_review.title = Some("Stored review title".to_string());
        store.create_review(&stored_review).unwrap();

        let timeline = build_branch_timeline(&store, &branch.id).unwrap();

        let title_of = |review_id: &str| {
            timeline
                .reviews
                .iter()
                .find(|r| r.id == review_id)
                .unwrap()
                .title
                .clone()
        };
        assert_eq!(
            title_of(&untitled_review.id),
            Some("Review: token refresh".to_string())
        );
        assert_eq!(
            title_of(&stored_review.id),
            Some("Stored review title".to_string())
        );
    }

    #[test]
    fn build_branch_timeline_completed_artifacts_ignore_acp_title() {
        let (repo, visible_sha, _stale_sha) = repo_with_visible_and_stale_commit();
        let (store, branch) = store_with_branch(&repo);

        let session = running_session(&store, "the prompt", Some("Interim ACP title"));
        store
            .update_session_status(&session.id, SessionStatus::Completed, None, None)
            .unwrap();

        let commit = Commit::new_with_sha(&branch.id, &visible_sha).with_session(&session.id);
        store.create_commit(&commit).unwrap();
        let note = Note::new(&branch.id, "Final note title", "note body").with_session(&session.id);
        store.create_note(&note).unwrap();
        let review =
            Review::new(&branch.id, &visible_sha, ReviewScope::Commit).with_session(&session.id);
        store.create_review(&review).unwrap();

        let timeline = build_branch_timeline(&store, &branch.id).unwrap();

        let landed = timeline
            .commits
            .iter()
            .find(|c| c.sha == visible_sha)
            .unwrap();
        assert_eq!(landed.subject, "visible change");
        assert_eq!(timeline.notes[0].title, "Final note title");
        assert_eq!(timeline.reviews[0].title, None);
    }

    // ── timeline ordering across a rebase ───────────────────────────────

    fn parsed_commit(line: &str) -> CommitTimelineItem {
        let store = Arc::new(Store::in_memory().unwrap());
        let commits = parse_commit_lines(&store, "branch-1", &[line.to_string()]);
        commits.into_iter().next().unwrap()
    }

    #[test]
    fn parse_commit_lines_takes_its_timestamp_from_author_time() {
        let commit = parsed_commit(
            "abc123\x1fabc123a\x1fTest\x1ftest@example.com\x1f9100\x1f1100\x1ffeat: parser",
        );

        assert_eq!(commit.subject, "feat: parser");
        assert_eq!(commit.timestamp, 1100);
        assert_eq!(commit.sort_timestamp, 1100);
    }

    fn commit_at(subject: &str, timestamp: i64, order: i64) -> CommitTimelineItem {
        CommitTimelineItem {
            id: None,
            sha: format!("sha-{order}"),
            short_sha: format!("sha-{order}"),
            subject: subject.to_string(),
            author: "Test".to_string(),
            author_email: "test@example.com".to_string(),
            timestamp,
            sort_timestamp: timestamp,
            order,
            session_id: None,
            session_status: None,
            completion_reason: None,
            is_own_commit: false,
        }
    }

    /// A cherry-picked commit keeps its original author date, which would sort
    /// it above the commit it actually follows. The clamp pins it down instead.
    #[test]
    fn clamp_keeps_out_of_order_author_dates_in_branch_order() {
        // Newest-first, as `git log` emits.
        let mut commits = vec![
            commit_at("fix: lexer", 300, 2),
            commit_at("chore: cherry-picked", 100, 1),
            commit_at("feat: parser", 200, 0),
        ];

        clamp_commit_sort_timestamps(&mut commits);

        assert_eq!(
            commits.iter().map(|c| c.sort_timestamp).collect::<Vec<_>>(),
            vec![300, 200, 200]
        );
        assert_eq!(
            commits.iter().map(|c| c.timestamp).collect::<Vec<_>>(),
            vec![300, 100, 200],
            "the rendered author dates stay untouched"
        );
    }

    #[test]
    fn clamp_leaves_already_increasing_author_dates_alone() {
        let mut commits = vec![commit_at("fix: lexer", 300, 1), commit_at("feat", 200, 0)];

        clamp_commit_sort_timestamps(&mut commits);

        assert_eq!(
            commits.iter().map(|c| c.sort_timestamp).collect::<Vec<_>>(),
            vec![300, 200]
        );
    }

    /// Commit the working tree with a fixed author date, leaving the committer
    /// date at "now" — the same split a rebase creates.
    fn commit_authored_at(repo: &TempGitRepo, message: &str, author_epoch: i64) -> String {
        repo.run_git(&["add", "."]);
        repo.run_git(&[
            "commit",
            "--date",
            &format!("@{author_epoch} +0000"),
            "-m",
            message,
        ]);
        repo.run_git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn committer_time(repo: &TempGitRepo, sha: &str) -> i64 {
        repo.run_git(&["show", "-s", "--format=%ct", sha])
            .trim()
            .parse()
            .unwrap()
    }

    /// The timeline interleaves commits with notes by timestamp. A rebase
    /// rewrites every committer date to "now" while notes keep their DB times,
    /// so sorting on committer time would sink every commit below every note.
    /// Author dates survive the rewrite, so the interleaving does too.
    #[test]
    fn build_branch_timeline_keeps_notes_interleaved_across_a_rebase() {
        const FIRST_AUTHORED_AT: i64 = 1_700_000_000;
        const NOTE_WRITTEN_AT: i64 = 1_700_000_100;
        const SECOND_AUTHORED_AT: i64 = 1_700_000_200;

        let repo = TempGitRepo::new();
        repo.write_file("base.txt", "base\n");
        let base_sha = repo.commit("chore: base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &base_sha]);

        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("parser.txt", "parser\n");
        commit_authored_at(&repo, "feat: parser", FIRST_AUTHORED_AT);
        repo.write_file("lexer.txt", "lexer\n");
        let old_head = commit_authored_at(&repo, "fix: lexer", SECOND_AUTHORED_AT);

        let (store, branch) = store_with_branch(&repo);
        let mut note = Note::new(&branch.id, "Plan", "the plan");
        note.created_at = NOTE_WRITTEN_AT * 1000;
        note.updated_at = note.created_at;
        note.completed_at = Some(note.created_at);
        store.create_note(&note).unwrap();

        let before = build_branch_timeline(&store, &branch.id).unwrap();
        assert_eq!(
            timeline_order(&before),
            vec!["feat: parser", "Plan", "fix: lexer"],
            "the note was written between the two commits"
        );

        // Move the base out from under the branch, then really rebase onto it.
        repo.run_git(&["checkout", "main"]);
        repo.write_file("moved.txt", "moved\n");
        let moved_base = repo.commit("chore: move base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &moved_base]);
        repo.run_git(&["checkout", "feature"]);
        repo.run_git(&["rebase", "--signoff", "origin/main"]);
        let new_head = repo.run_git(&["rev-parse", "HEAD"]).trim().to_string();
        assert_ne!(new_head, old_head, "the rebase must rewrite the SHAs");

        let after = build_branch_timeline(&store, &branch.id).unwrap();

        assert_eq!(
            timeline_order(&after),
            vec!["feat: parser", "Plan", "fix: lexer"],
            "the note must stay between the two commits after the rebase"
        );
        assert_eq!(
            after
                .commits
                .iter()
                .map(|c| c.timestamp)
                .collect::<Vec<_>>(),
            vec![SECOND_AUTHORED_AT, FIRST_AUTHORED_AT],
            "the rewritten commits keep their original author dates"
        );
        // The committer dates really are elsewhere — a `%ct` sort would put
        // both commits after the note rather than around it.
        assert!(
            committer_time(&repo, &new_head) > NOTE_WRITTEN_AT,
            "committer time is 'now', long after the note was written"
        );
    }

    /// Commit subjects and note titles, merged and sorted the way the frontend
    /// does it (`BranchTimeline.svelte`): ascending sort timestamp, `order`
    /// breaking ties between commits.
    fn timeline_order(timeline: &BranchTimeline) -> Vec<&str> {
        let mut items: Vec<(i64, i64, &str)> = timeline
            .commits
            .iter()
            .map(|c| (c.sort_timestamp, c.order, c.subject.as_str()))
            .chain(timeline.notes.iter().map(|n| {
                (
                    n.completed_at.unwrap_or(n.created_at) / 1000,
                    0,
                    n.title.as_str(),
                )
            }))
            .collect();
        items.sort_by_key(|(timestamp, order, _)| (*timestamp, *order));
        items.into_iter().map(|(_, _, label)| label).collect()
    }
}
