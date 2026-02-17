//! Tauri commands for session management.
//!
//! Separated from `lib.rs` to keep session concerns isolated. These are
//! the commands exposed to the frontend via IPC.
//!
//! ## Design note: minimal surface area
//!
//! Only commands the frontend legitimately needs are exposed here:
//! - `start_session` / `resume_session` — kick off agent work
//! - `start_branch_session` — kick off branch-scoped agent work (note/commit)
//! - `cancel_session` / `delete_session` — lifecycle control
//! - `get_session` / `get_session_messages` / `get_session_messages_since` — reads for polling
//!
//! Internal-only operations (creating bare sessions, inserting messages,
//! updating status) are **not** exposed as Tauri commands. They're used
//! only by the backend (`session_runner` / `agent` modules) via the
//! `Store` directly.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::agent::{self, AcpProviderInfo};
use crate::blox;
use crate::git;
use crate::session_runner::{self, SessionConfig};
use crate::store::{self, Store};

// =============================================================================
// Helper — duplicated from lib.rs to avoid circular deps. If this grows,
// consider extracting a shared `state.rs`.
// =============================================================================

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

fn resolve_branch_repo_slug(
    store: &Arc<Store>,
    project: &store::Project,
    branch: &store::Branch,
) -> Option<String> {
    if let Some(repo_id) = &branch.project_repo_id {
        if let Ok(Some(repo)) = store.get_project_repo(repo_id) {
            return Some(repo.github_repo);
        }
    }
    project.primary_repo().map(|s| s.to_string())
}

// =============================================================================
// Provider discovery
// =============================================================================

/// Scan the system for installed ACP-compatible agents.
///
/// Returns a list of available providers with their IDs and labels.
/// The frontend uses this for the agent setup modal and selector.
///
/// Marked `async` so Tauri runs it on the async runtime instead of the
/// main thread — `find_command` spawns login shells which can take tens
/// of milliseconds per agent.
#[tauri::command]
pub async fn discover_acp_providers() -> Vec<AcpProviderInfo> {
    tokio::task::spawn_blocking(agent::discover_providers)
        .await
        .unwrap_or_default()
}

// =============================================================================
// Read-only queries (used by frontend polling)
// =============================================================================

#[tauri::command]
pub fn get_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
) -> Result<Option<store::Session>, String> {
    get_store(&store)?
        .get_session(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_session_messages(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
) -> Result<Vec<store::SessionMessage>, String> {
    get_store(&store)?
        .get_session_messages(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_session_messages_since(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
    since_id: i64,
) -> Result<Vec<store::SessionMessage>, String> {
    get_store(&store)?
        .get_session_messages_since(&session_id, since_id)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Lifecycle commands
// =============================================================================

/// Create a session and immediately start the agent.
///
/// The prompt is persisted as the first user message, goose is spawned
/// in the background, and messages stream into the DB in real-time.
/// Returns the Session record (status will be "running").
#[tauri::command]
pub fn start_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    prompt: String,
    working_dir: String,
    provider: Option<String>,
) -> Result<store::Session, String> {
    let store = get_store(&store)?;
    let working_dir = PathBuf::from(working_dir);
    let mut session = store::Session::new_running(&prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    session_runner::start_session(
        SessionConfig {
            session_id: session.id.clone(),
            prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name: None,
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(session)
}

/// Send a follow-up message to an existing session.
///
/// Sets the session status back to "running", persists the user message,
/// and spawns a new goose subprocess. Uses ACP `load_session` to restore
/// the agent's conversation history from the previous turn(s), so the
/// agent has full context when processing the follow-up.
///
/// The working directory is read from the session record (set when the
/// session was first created), so the frontend doesn't need to pass it.
#[tauri::command]
pub fn resume_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    session_id: String,
    prompt: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    let session = store
        .get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Session not found: {session_id}"))?;

    // Use the provider that originally created this session so the
    // agent's conversation history can be restored correctly.
    let provider = session.provider.clone();
    let agent_session_id = session.agent_id.clone();
    let working_dir = PathBuf::from(&session.working_dir);

    let transitioned = store
        .transition_to_running(&session_id)
        .map_err(|e| e.to_string())?;
    if !transitioned {
        return Err("Session is already running".to_string());
    }

    let _ = app_handle.emit(
        "session-status-changed",
        session_runner::SessionStatusEvent {
            session_id: session_id.clone(),
            status: "running".to_string(),
            error_message: None,
        },
    );

    session_runner::start_session(
        SessionConfig {
            session_id,
            prompt,
            working_dir,
            agent_session_id,
            pre_head_sha: None,
            provider,
            workspace_name: None,
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(())
}

#[tauri::command]
pub fn cancel_session(
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    session_id: String,
) -> Result<(), String> {
    let was_running = registry.cancel(&session_id);
    if !was_running {
        let store = get_store(&store)?;
        if let Ok(Some(session)) = store.get_session(&session_id) {
            if session.status == store::SessionStatus::Running {
                let _ =
                    store.update_session_status(&session_id, store::SessionStatus::Cancelled, None);
                let _ = app_handle.emit(
                    "session-status-changed",
                    session_runner::SessionStatusEvent {
                        session_id: session_id.clone(),
                        status: "cancelled".to_string(),
                        error_message: None,
                    },
                );
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn delete_session(
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
) -> Result<(), String> {
    registry.cancel(&session_id);

    get_store(&store)?
        .delete_session(&session_id)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Branch-scoped sessions (note / commit)
// =============================================================================

/// The type of branch session to start.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BranchSessionType {
    Note,
    Commit,
    Review,
}

/// Response from starting a branch session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSessionResponse {
    pub session_id: String,
    /// The ID of the artifact created (commit or note).
    pub artifact_id: String,
}

/// Start a branch-scoped session (note or commit).
///
/// This builds the full prompt (action tag + branch history + user prompt),
/// creates the artifact stub, and kicks off the agent in the branch's workdir.
///
/// For remote branches (those with a `workspace_name`), the session runs via
/// `blox acp` instead of a local agent binary. Branch context and commit
/// detection are skipped since there is no local worktree.
#[tauri::command(rename_all = "camelCase")]
pub fn start_branch_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    prompt: String,
    session_type: BranchSessionType,
    provider: Option<String>,
) -> Result<BranchSessionResponse, String> {
    let store = get_store(&store)?;

    // Resolve branch → project
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let is_remote = branch.workspace_name.is_some();

    // Resolve working directory and branch context.
    // Remote branches use ws_exec for git operations; local branches use the worktree directly.
    let (working_dir, branch_context) = if is_remote {
        // For remote branches, use the derived clone path as a fallback working dir.
        // The actual work happens via ws_exec, not local filesystem.
        let fallback_dir = resolve_branch_repo_slug(&store, &project, &branch)
            .and_then(|repo| crate::paths::repos_dir().map(|d| d.join(repo)))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let ctx = build_remote_branch_context(
            branch.workspace_name.as_deref().unwrap(),
            &branch.base_branch,
            &store,
            &branch_id,
        );
        (fallback_dir, ctx)
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut worktree_path = PathBuf::from(&workdir.path);
        if let Some(ref subpath) = project.subpath {
            worktree_path = worktree_path.join(subpath);
        }

        let ctx = build_branch_context(&worktree_path, &branch.base_branch, &store, &branch_id);
        (worktree_path, ctx)
    };

    // Build the full prompt with action tag + context
    let full_prompt = build_full_prompt(&prompt, &branch_context, &session_type);

    // Create the session
    let mut session = store::Session::new_running(&full_prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Create artifact stub and compute pre-head SHA
    let (artifact_id, pre_head_sha) = match session_type {
        BranchSessionType::Note => {
            let note = store::Note::new(&branch_id, &prompt, "").with_session(&session.id);
            store.create_note(&note).map_err(|e| e.to_string())?;
            (note.id, None)
        }
        BranchSessionType::Commit => {
            let commit = store::Commit::new_pending(&branch_id).with_session(&session.id);
            store.create_commit(&commit).map_err(|e| e.to_string())?;
            // For remote branches, get HEAD via ws_exec; for local, use git directly.
            let head_sha = if is_remote {
                match blox::ws_exec(
                    branch.workspace_name.as_deref().unwrap(),
                    &["git", "rev-parse", "HEAD"],
                ) {
                    Ok(sha) => Some(sha.trim().to_string()),
                    Err(e) => {
                        log::warn!("Failed to get remote HEAD SHA via ws_exec: {e}");
                        None
                    }
                }
            } else {
                Some(
                    git::get_head_sha(&working_dir)
                        .map_err(|e| format!("Failed to get HEAD SHA: {e}"))?,
                )
            };
            (commit.id, head_sha)
        }
        BranchSessionType::Review => {
            // Get the current tip SHA for the review anchor
            let tip_sha = if is_remote {
                blox::ws_exec(
                    branch.workspace_name.as_deref().unwrap(),
                    &["git", "rev-parse", "HEAD"],
                )
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
            } else {
                git::get_head_sha(&working_dir)
                    .map_err(|e| format!("Failed to get HEAD SHA: {e}"))?
            };

            let review = store::Review::new(&branch_id, &tip_sha, store::ReviewScope::Branch)
                .with_session(&session.id);
            store.create_review(&review).map_err(|e| e.to_string())?;
            (review.id, None)
        }
    };

    // For remote branches, use the user's UI selection.
    let effective_provider = provider;

    session_runner::start_session(
        SessionConfig {
            session_id: session.id.clone(),
            prompt: full_prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha,
            provider: effective_provider,
            workspace_name: branch.workspace_name.clone(),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(BranchSessionResponse {
        session_id: session.id,
        artifact_id,
    })
}

// =============================================================================
// Prompt construction helpers
// =============================================================================

/// Build the branch history context block for a local branch.
fn build_branch_context(
    worktree: &Path,
    base_branch: &str,
    store: &Arc<Store>,
    branch_id: &str,
) -> String {
    let mut parts = vec![context_preamble()];
    let mut timeline: Vec<TimelineEntry> = Vec::new();
    let mut commit_error = None;

    // Commits from git log
    match git::get_full_commit_log(worktree, base_branch) {
        Ok(log) if !log.trim().is_empty() => {
            timeline.extend(parse_timestamped_log(&log));
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("Failed to get commit log for branch context: {e}");
            commit_error = Some(format!("(Error retrieving commit log: {e})"));
        }
    }

    // Notes and reviews from DB
    timeline.extend(note_timeline_entries(store, branch_id, false));
    timeline.extend(review_timeline_entries(store, branch_id));

    parts.push(render_timeline(timeline, commit_error));
    parts.join("\n\n")
}

/// Build the branch history context block for a remote branch.
///
/// Uses `blox ws_exec` to run git commands inside the remote workspace,
/// and reads notes from the DB (which works regardless of worktree location).
fn build_remote_branch_context(
    workspace_name: &str,
    base_branch: &str,
    store: &Arc<Store>,
    branch_id: &str,
) -> String {
    let mut parts = vec![context_preamble()];
    let mut timeline: Vec<TimelineEntry> = Vec::new();

    // Full commit log via ws_exec.
    // Use merge-base to find the fork point so that only the branch's own
    // commits are included, even after a rebase or when the base ref has
    // moved forward.
    let range = if let Ok(mb_output) =
        blox::ws_exec(workspace_name, &["git", "merge-base", base_branch, "HEAD"])
    {
        let mb = mb_output.trim().to_string();
        format!("{mb}..HEAD")
    } else {
        format!("{base_branch}..HEAD")
    };
    match blox::ws_exec(
        workspace_name,
        &[
            "git",
            "log",
            "--reverse",
            "--format=%x00%ct%x01commit %H%nAuthor: %an%nDate: %ci%n%n%B",
            &range,
        ],
    ) {
        Ok(log) if !log.trim().is_empty() => {
            timeline.extend(parse_timestamped_log(&log));
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("Failed to get remote commit log via ws_exec: {e}");
        }
    }

    // Notes (inlined — remote agent can't access local temp files) and reviews
    timeline.extend(note_timeline_entries(store, branch_id, true));
    timeline.extend(review_timeline_entries(store, branch_id));

    parts.push(render_timeline(timeline, None));
    parts.join("\n\n")
}

/// Shared preamble for branch context blocks.
fn context_preamble() -> String {
    "This branch represents an ongoing conversation across multiple sessions. \
     Be judicious with your context window, but you are responsible for understanding \
     previous changes or note content from the branch history when they relate to the \
     next step."
        .to_string()
}

// =============================================================================
// Chronological timeline helpers
// =============================================================================

/// A single entry in the branch timeline, sorted by timestamp.
struct TimelineEntry {
    timestamp: i64,
    content: String,
}

/// Sort timeline entries and render them into a single section.
fn render_timeline(mut timeline: Vec<TimelineEntry>, error: Option<String>) -> String {
    if timeline.is_empty() {
        let mut s = String::from("## Branch History\n\n");
        if let Some(err) = error {
            s.push_str(&err);
        } else {
            s.push_str("No activity on this branch yet.");
        }
        return s;
    }

    timeline.sort_by_key(|e| e.timestamp);

    let mut section = String::from("## Branch History (oldest first)\n");
    if let Some(err) = error {
        section.push_str(&format!("\n{err}\n"));
    }
    for entry in &timeline {
        section.push('\n');
        section.push_str(&entry.content);
        section.push('\n');
    }
    section
}

/// Parse a timestamped git log into timeline entries.
///
/// Expects the format produced by `--format=%x00%ct%x01commit %H…`:
/// `\0<unix_ts>\x01<display_text>` per commit.
fn parse_timestamped_log(output: &str) -> Vec<TimelineEntry> {
    let mut entries = Vec::new();
    for record in output.split('\0') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        if let Some((ts_str, display)) = record.split_once('\x01') {
            if let Ok(ts) = ts_str.trim().parse::<i64>() {
                entries.push(TimelineEntry {
                    timestamp: ts,
                    content: display.trim().to_string(),
                });
            }
        }
    }
    entries
}

/// Convert notes from the DB into timeline entries.
///
/// When `is_remote` is true, note content is inlined directly because the
/// remote agent cannot access local temp files. For local branches, notes
/// are written to temp files and referenced by path.
fn note_timeline_entries(
    store: &Arc<Store>,
    branch_id: &str,
    is_remote: bool,
) -> Vec<TimelineEntry> {
    let notes = match store.list_notes_for_branch(branch_id) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("Failed to list notes for branch context: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    for note in &notes {
        if note.content.is_empty() {
            continue; // skip notes still generating
        }
        let content = if is_remote {
            format!("### Note: {}\n\n{}", note.title, note.content)
        } else {
            let note_path = std::env::temp_dir().join(format!("staged-note-{}.md", note.id));
            if let Err(e) = std::fs::write(&note_path, &note.content) {
                log::warn!("Failed to write note to temp file: {e}");
                continue;
            }
            format!("### Note: {}\n\nSee: `{}`", note.title, note_path.display())
        };
        entries.push(TimelineEntry {
            timestamp: note.created_at,
            content,
        });
    }
    entries
}

/// Convert code reviews (with comments) from the DB into timeline entries.
fn review_timeline_entries(store: &Arc<Store>, branch_id: &str) -> Vec<TimelineEntry> {
    let reviews = match store.list_reviews_for_branch(branch_id) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Failed to list reviews for branch context: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    for review in &reviews {
        if review.comments.is_empty() {
            continue;
        }
        let short_sha = &review.commit_sha[..review.commit_sha.len().min(7)];
        let mut content = format!(
            "### Code Review of {} ({} scope)\n",
            short_sha,
            review.scope.as_str(),
        );

        // Group comments by file path
        let mut by_path: std::collections::BTreeMap<&str, Vec<&crate::store::models::Comment>> =
            std::collections::BTreeMap::new();
        for comment in &review.comments {
            by_path.entry(&comment.path).or_default().push(comment);
        }

        for (path, comments) in &by_path {
            for comment in comments {
                if comment.span.start == comment.span.end {
                    content.push_str(&format!(
                        "\n- **{}** (line {}): {}",
                        path, comment.span.start, comment.content,
                    ));
                } else {
                    content.push_str(&format!(
                        "\n- **{}** (lines {}–{}): {}",
                        path, comment.span.start, comment.span.end, comment.content,
                    ));
                }
            }
        }

        entries.push(TimelineEntry {
            timestamp: review.created_at,
            content,
        });
    }
    entries
}

/// Assemble the full prompt from action tag + branch context + user prompt.
fn build_full_prompt(
    user_prompt: &str,
    branch_context: &str,
    session_type: &BranchSessionType,
) -> String {
    let action_tag = match session_type {
        BranchSessionType::Note => {
            "\
<action>
The user is requesting a note. Generate a note based on their prompt below.

You may use any tools needed to research and gather information, but do NOT create \
any commits.

To return the note, include a horizontal rule (---) followed by the note content. \
Begin the note with a markdown H1 heading as the title.
</action>"
        }
        BranchSessionType::Commit => {
            "\
<action>
The user is requesting you make a commit based on the prompt below. Make the necessary \
code changes, following any verification or formatting steps as instructed, and then \
create a commit with a conventional commit message. This commit should describe what \
was requested and how it was fulfilled.
</action>"
        }
        BranchSessionType::Review => {
            "\
<action>
The user is requesting an AI code review of the current branch.

Review the code changes on this branch by running `git diff $(git merge-base origin/HEAD HEAD)..HEAD` \
(or the appropriate base branch) and provide feedback as structured comments. \
Focus on: correctness, potential bugs, readability, and adherence to best practices.

Do NOT create any commits or modify any files.

Return your review as a JSON block fenced with ```review-comments markers:

```review-comments
[
  {
    \"path\": \"src/foo.ts\",
    \"span\": { \"start\": 10, \"end\": 15 },
    \"content\": \"This function doesn't handle the null case...\"
  }
]
```

The `span` uses 0-indexed line numbers from the \"after\" side of the diff (exclusive end). \
Only comment on changed files. Be specific and actionable.
</action>"
        }
    };

    format!(
        "{action_tag}\n\n\
         <branch-history>\n\
         {branch_context}\n\
         </branch-history>\n\n\
         {user_prompt}"
    )
}
