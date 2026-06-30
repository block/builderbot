//! Tauri commands for session management.
//!
//! Separated from `lib.rs` to keep session concerns isolated. These are
//! the commands exposed to the frontend via IPC.
//!
//! ## Design note: minimal surface area
//!
//! Only commands the frontend legitimately needs are exposed here:
//! - `start_session` / `resume_session` — kick off agent work
//! - `start_or_queue_branch_session` — start or enqueue branch-scoped agent work
//! - `cancel_session` / `delete_session` — lifecycle control
//! - `get_session` / `get_session_messages` / `get_session_messages_since` — reads for polling
//!
//! Internal-only operations (creating bare sessions, inserting messages,
//! updating status) are **not** exposed as Tauri commands. They're used
//! only by the backend (`session_runner` / `agent` modules) via the
//! `Store` directly.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::path::BaseDirectory;
use tauri::Manager;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::agent::{self, AcpProviderInfo};
use crate::blox;
use crate::git;
use crate::session_runner::{self, SessionConfig};
use crate::store::{self, Store};

const PIKCHR_GRAMMAR_RESOURCE: &str = "resources/pikchr/grammar.md";
const PIKCHR_GRAMMAR_REMOTE_PATH_PREFIX: &str = "/tmp/staged-pikchr-grammar-";
const PIKCHR_GRAMMAR_REMOTE_PATH_SUFFIX: &str = ".md";
pub(crate) const PIKCHR_GRAMMAR_URL: &str = "https://pikchr.org/home/doc/trunk/doc/grammar.md";

enum RemotePikchrGrammarStaging {
    NotNeeded,
    Upload { bytes: Vec<u8>, remote_path: String },
    FallbackUrl,
}

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

pub(crate) fn resolve_branch_repo_slug(
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

pub(crate) async fn run_blox_blocking<T, F>(op: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, blox::BloxError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(op)
        .await
        .map_err(|e| format!("blox task failed: {e}"))?
        .map_err(|e| e.to_string())
}

fn bundled_pikchr_grammar_path(app_handle: &tauri::AppHandle) -> Option<PathBuf> {
    if let Ok(path) = app_handle
        .path()
        .resolve(PIKCHR_GRAMMAR_RESOURCE, BaseDirectory::Resource)
    {
        if path.is_file() {
            return Some(path);
        }
    }

    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(PIKCHR_GRAMMAR_RESOURCE);
    if source_path.is_file() {
        return Some(source_path);
    }

    None
}

fn bundled_pikchr_grammar_bytes(app_handle: &tauri::AppHandle) -> Option<Vec<u8>> {
    let Some(grammar_path) = bundled_pikchr_grammar_path(app_handle) else {
        log::warn!("Bundled Pikchr grammar resource not found; falling back to public URL");
        return None;
    };

    match std::fs::read(&grammar_path) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            log::warn!(
                "Failed to read bundled Pikchr grammar at {}: {e}",
                grammar_path.display()
            );
            None
        }
    }
}

fn generated_pikchr_grammar_remote_path() -> String {
    format!(
        "{PIKCHR_GRAMMAR_REMOTE_PATH_PREFIX}{}{PIKCHR_GRAMMAR_REMOTE_PATH_SUFFIX}",
        uuid::Uuid::new_v4()
    )
}

fn remote_pikchr_grammar_staging(
    app_handle: &tauri::AppHandle,
    session_type: &BranchSessionType,
) -> RemotePikchrGrammarStaging {
    if !matches!(session_type, BranchSessionType::Note) {
        return RemotePikchrGrammarStaging::NotNeeded;
    }

    match bundled_pikchr_grammar_bytes(app_handle) {
        Some(bytes) => RemotePikchrGrammarStaging::Upload {
            bytes,
            remote_path: generated_pikchr_grammar_remote_path(),
        },
        None => RemotePikchrGrammarStaging::FallbackUrl,
    }
}

fn local_pikchr_grammar_reference_for_session(
    app_handle: &tauri::AppHandle,
    session_type: &BranchSessionType,
) -> String {
    if matches!(session_type, BranchSessionType::Note) {
        resolve_pikchr_grammar_reference(app_handle, None)
    } else {
        PIKCHR_GRAMMAR_URL.to_string()
    }
}

fn upload_pikchr_grammar_to_remote_with_writer<F>(
    workspace_name: &str,
    bytes: &[u8],
    remote_path: String,
    write: F,
) -> String
where
    F: FnOnce(&str, &[u8], &str) -> Result<(), String>,
{
    match write(workspace_name, bytes, &remote_path) {
        Ok(()) => remote_path,
        Err(e) => {
            log::warn!("Failed to copy Pikchr grammar to remote workspace {workspace_name}: {e}");
            PIKCHR_GRAMMAR_URL.to_string()
        }
    }
}

fn upload_pikchr_grammar_to_remote(
    workspace_name: &str,
    bytes: &[u8],
    remote_path: String,
) -> String {
    upload_pikchr_grammar_to_remote_with_writer(
        workspace_name,
        bytes,
        remote_path,
        write_bytes_to_remote,
    )
}

pub(crate) fn resolve_pikchr_grammar_reference(
    app_handle: &tauri::AppHandle,
    workspace_name: Option<&str>,
) -> String {
    let Some(grammar_path) = bundled_pikchr_grammar_path(app_handle) else {
        log::warn!("Bundled Pikchr grammar resource not found; falling back to public URL");
        return PIKCHR_GRAMMAR_URL.to_string();
    };

    if let Some(workspace_name) = workspace_name {
        return match std::fs::read(&grammar_path) {
            Ok(bytes) => upload_pikchr_grammar_to_remote(
                workspace_name,
                &bytes,
                generated_pikchr_grammar_remote_path(),
            ),
            Err(e) => {
                log::warn!(
                    "Failed to read bundled Pikchr grammar at {}: {e}",
                    grammar_path.display()
                );
                PIKCHR_GRAMMAR_URL.to_string()
            }
        };
    }

    grammar_path.to_string_lossy().into_owned()
}

fn pikchr_note_guidance(reference: &str) -> String {
    format!(
        "Staged notes support rendered diagrams in fenced `pikchr` code blocks. \
If you need the Pikchr grammar while writing a diagram, read the reference at: {reference}"
    )
}

pub(crate) fn build_note_followup_message_with_pikchr_reference(
    has_parsed_note: bool,
    pikchr_grammar_reference: &str,
) -> String {
    let visible_request = if has_parsed_note {
        "Please update the note to reflect the latest chat."
    } else {
        "Please write the note for this session."
    };
    let linked_note_action = if has_parsed_note {
        "update the linked note"
    } else {
        "write the linked note"
    };
    let pikchr_guidance = pikchr_note_guidance(pikchr_grammar_reference);

    format!(
        "<action>\n\
The user is asking you to {linked_note_action} from the latest chat history.\n\
\n\
Use the existing conversation context. Do not create commits.\n\
\n\
{pikchr_guidance}\n\
\n\
Your final response must include a suggested-next-steps fenced block followed by the note content after a horizontal rule:\n\
\n\
```suggested-next-steps\n\
{{\"suggestedNextCommitStep\": null, \"suggestedNextNoteStep\": null}}\n\
```\n\
\n\
---\n\
# <Title>\n\
<Body>\n\
\n\
Formatting requirements:\n\
- The opening fence line for suggested-next-steps must be exactly: ```suggested-next-steps\n\
- The closing fence line must be exactly: ```\n\
- Put only a JSON object inside the suggested-next-steps block.\n\
- Include both nullable string fields: suggestedNextCommitStep and suggestedNextNoteStep.\n\
- Keep suggested next steps concise; use null when there is no clear next action.\n\
- The `---` separator must be on its own line.\n\
- The note content must start immediately after `---` with a markdown H1.\n\
- Do not wrap the note in code fences.\n\
</action>\n\
\n\
{visible_request}"
    )
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

#[tauri::command]
pub fn count_assistant_messages_after(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
    after_timestamp: i64,
) -> Result<i64, String> {
    get_store(&store)?
        .count_assistant_messages_after(&session_id, after_timestamp)
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
pub async fn start_session(
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
            extra_env: vec![],
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir: None,
            image_ids: vec![],
            branch_id: None,
            project_id: None,
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
#[allow(clippy::too_many_arguments)]
pub async fn resume_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    action_executor: tauri::State<'_, Arc<ActionExecutor>>,
    action_registry: tauri::State<'_, Arc<ActionRegistry>>,
    app_handle: tauri::AppHandle,
    session_id: String,
    prompt: String,
    image_ids: Option<Vec<String>>,
    branch_id: Option<String>,
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

    // Check if this session is linked to a project note — if so, we need
    // to start the MCP server so the agent has access to project tools.
    let project_note = store
        .get_project_note_by_session(&session_id)
        .ok()
        .flatten();
    let mcp_project_id = project_note.as_ref().map(|note| note.project_id.clone());
    let linked_commit = store.get_commit_by_session(&session_id).ok().flatten();
    let linked_note = store.get_note_by_session(&session_id).ok().flatten();
    let linked_review = store.get_review_by_session(&session_id).ok().flatten();

    // If a branch_id is provided, look up the branch to get workspace_name
    // and resolve remote_working_dir. This takes priority over the
    // commit-based fallback below.
    let branch_from_id = branch_id
        .as_deref()
        .and_then(|bid| store.get_branch(bid).ok().flatten());

    let linked_branch = if branch_from_id.is_some() {
        branch_from_id.clone()
    } else if let Some(commit) = &linked_commit {
        store.get_branch(&commit.branch_id).ok().flatten()
    } else if let Some(note) = &linked_note {
        store.get_branch(&note.branch_id).ok().flatten()
    } else if let Some(review) = &linked_review {
        store.get_branch(&review.branch_id).ok().flatten()
    } else {
        None
    };

    let session_type = if project_note.is_some() {
        // Project notes and branch notes intentionally share the "note"
        // session type because the frontend only needs a single "note work is
        // running" signal for project-level activity indicators.
        Some("note".to_string())
    } else if linked_commit.is_some() {
        Some("commit".to_string())
    } else if linked_note.is_some() {
        Some("note".to_string())
    } else if linked_review.is_some() {
        Some("review".to_string())
    } else {
        infer_branch_resume_session_type(&session.prompt).map(str::to_string)
    };
    let event_branch_id = linked_branch.as_ref().map(|branch| branch.id.clone());
    let event_project_id = if let Some(note) = &project_note {
        Some(note.project_id.clone())
    } else {
        linked_branch
            .as_ref()
            .map(|branch| branch.project_id.clone())
    };

    // Only resumed commit sessions need a pre-run HEAD snapshot. The
    // completion hook ignores non-commit sessions anyway, but keeping this
    // narrow makes the intent explicit and avoids unnecessary git lookups.
    let (pre_head_sha, workspace_name) = {
        if let Some(ref branch) = linked_branch {
            let ws_name = branch.workspace_name.clone();
            let head = if linked_commit.is_some() {
                if let Some(ref ws) = ws_name {
                    let ws = ws.clone();
                    run_blox_blocking(move || {
                        crate::blox::ws_exec(&ws, &["git", "rev-parse", "HEAD"])
                    })
                    .await
                    .map(|s| s.trim().to_string())
                    .ok()
                } else {
                    crate::git::get_head_sha(&working_dir).ok()
                }
            } else {
                None
            };
            (head, ws_name)
        } else {
            (None, None)
        }
    };

    // For remote branches, resolve the actual workspace path so the remote
    // agent starts in the correct repo directory.
    let remote_working_dir = if let Some(ref branch) = branch_from_id {
        if branch.workspace_name.is_some() {
            let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
            let store_for_resolve = Arc::clone(&store);
            let branch_for_resolve = branch.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                crate::branches::resolve_branch_workspace_subpath(
                    &store_for_resolve,
                    &branch_for_resolve,
                )
                .ok()
                .flatten()
                .and_then(|subpath| {
                    crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
                })
            })
            .await
            {
                Ok(Some(path)) => Some(PathBuf::from(path)),
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    };

    let transitioned = store
        .transition_to_running(&session_id)
        .map_err(|e| e.to_string())?;
    if !transitioned {
        return Err("Session is already running".to_string());
    }

    let config_branch_id = event_branch_id.clone();
    let config_project_id = event_project_id.clone().or(mcp_project_id.clone());

    crate::web_server::emit_to_all(
        &app_handle,
        "session-status-changed",
        session_runner::SessionStatusEvent {
            session_id: session_id.clone(),
            status: "running".to_string(),
            error_message: None,
            completion_reason: None,
            branch_id: event_branch_id,
            project_id: event_project_id.or(mcp_project_id.clone()),
            session_type,
            is_auto_review: false,
        },
    );

    session_runner::start_session(
        SessionConfig {
            session_id,
            prompt,
            working_dir,
            agent_session_id,
            pre_head_sha,
            provider,
            workspace_name,
            extra_env: if linked_commit.is_some() {
                session_runner::git_identity_env_from_global_config()
            } else {
                vec![]
            },
            mcp_project_id: mcp_project_id.clone(),
            action_executor: if mcp_project_id.is_some() {
                Some(Arc::clone(&action_executor))
            } else {
                None
            },
            action_registry: if mcp_project_id.is_some() {
                Some(Arc::clone(&action_registry))
            } else {
                None
            },
            remote_working_dir,
            image_ids: image_ids.unwrap_or_default(),
            branch_id: config_branch_id,
            project_id: config_project_id,
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(())
}

pub(crate) fn infer_branch_resume_session_type(prompt: &str) -> Option<&'static str> {
    // Keep these checks aligned with the action prompts built in `prs.rs`.
    if prompt.contains("Create a draft pull request for the current branch.")
        || prompt.contains("Create a pull request for the current branch.")
    {
        Some("pr")
    } else if prompt.contains("Push the current branch to the remote using force-with-lease.")
        || prompt.contains("Push the current branch to the remote.")
    {
        Some("push")
    } else {
        None
    }
}

#[tauri::command]
pub async fn build_note_followup_message(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    session_id: String,
    branch_id: Option<String>,
    has_parsed_note: bool,
) -> Result<String, String> {
    let store = get_store(&store)?;

    tauri::async_runtime::spawn_blocking(move || {
        store
            .get_session(&session_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Session not found: {session_id}"))?;

        let linked_commit = store.get_commit_by_session(&session_id).ok().flatten();
        let linked_note = store.get_note_by_session(&session_id).ok().flatten();
        let linked_review = store.get_review_by_session(&session_id).ok().flatten();

        let branch_from_id = branch_id
            .as_deref()
            .and_then(|bid| store.get_branch(bid).ok().flatten());

        let linked_branch = if branch_from_id.is_some() {
            branch_from_id
        } else if let Some(commit) = &linked_commit {
            store.get_branch(&commit.branch_id).ok().flatten()
        } else if let Some(note) = &linked_note {
            store.get_branch(&note.branch_id).ok().flatten()
        } else if let Some(review) = &linked_review {
            store.get_branch(&review.branch_id).ok().flatten()
        } else {
            None
        };

        let pikchr_grammar_reference = resolve_pikchr_grammar_reference(
            &app_handle,
            linked_branch
                .as_ref()
                .and_then(|branch| branch.workspace_name.as_deref()),
        );

        Ok(build_note_followup_message_with_pikchr_reference(
            has_parsed_note,
            &pikchr_grammar_reference,
        ))
    })
    .await
    .map_err(|e| format!("Failed to build note follow-up message: {e}"))?
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
            if session.status == store::SessionStatus::Running
                || session.status == store::SessionStatus::Queued
            {
                let _ = store.update_session_status(
                    &session_id,
                    store::SessionStatus::Cancelled,
                    None,
                    Some(&store::CompletionReason::Interrupted),
                );
                let branch_id = store.get_branch_id_for_session(&session_id).ok().flatten();
                let project_id = store.get_project_id_for_session(&session_id).ok().flatten();
                crate::web_server::emit_to_all(
                    &app_handle,
                    "session-status-changed",
                    session_runner::SessionStatusEvent {
                        session_id: session_id.clone(),
                        status: "cancelled".to_string(),
                        error_message: None,
                        completion_reason: Some("interrupted".to_string()),
                        branch_id,
                        project_id,
                        session_type: None,
                        is_auto_review: false,
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
// Branch-scoped sessions (note / commit / review)
// =============================================================================

/// The type of branch session to start.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BranchSessionType {
    Note,
    Commit,
    Review,
}

impl BranchSessionType {
    fn as_str(&self) -> &'static str {
        match self {
            BranchSessionType::Commit => "commit",
            BranchSessionType::Note => "note",
            BranchSessionType::Review => "review",
        }
    }

    fn schedule_kind(&self) -> BranchSessionScheduleKind {
        match self {
            BranchSessionType::Commit => BranchSessionScheduleKind::Commit,
            BranchSessionType::Note => BranchSessionScheduleKind::Note,
            BranchSessionType::Review => BranchSessionScheduleKind::Review,
        }
    }
}

fn extra_env_for_branch_session(session_type: &BranchSessionType) -> Vec<(String, String)> {
    if matches!(session_type, BranchSessionType::Commit) {
        session_runner::git_identity_env_from_global_config()
    } else {
        vec![]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BranchSessionScheduleKind {
    Commit,
    Note,
    Review,
    CommitPipeline,
}

impl BranchSessionScheduleKind {
    fn is_exclusive(self) -> bool {
        matches!(
            self,
            BranchSessionScheduleKind::Commit | BranchSessionScheduleKind::CommitPipeline
        )
    }

    fn allows_parallel_instances(self) -> bool {
        matches!(self, BranchSessionScheduleKind::Note)
    }

    fn branch_session_type(self) -> Option<BranchSessionType> {
        match self {
            BranchSessionScheduleKind::Commit => Some(BranchSessionType::Commit),
            BranchSessionScheduleKind::Note => Some(BranchSessionType::Note),
            BranchSessionScheduleKind::Review => Some(BranchSessionType::Review),
            BranchSessionScheduleKind::CommitPipeline => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BranchSessionSchedule {
    kind: BranchSessionScheduleKind,
    review_id: Option<String>,
    blocks_queue: bool,
}

fn can_start_with_active_branch_sessions(
    candidate: BranchSessionScheduleKind,
    active: &HashSet<BranchSessionScheduleKind>,
) -> bool {
    if candidate.is_exclusive() {
        return active.is_empty();
    }

    if active.iter().any(|kind| kind.is_exclusive()) {
        return false;
    }

    candidate.allows_parallel_instances() || !active.contains(&candidate)
}

fn note_session_schedule() -> BranchSessionSchedule {
    BranchSessionSchedule {
        kind: BranchSessionScheduleKind::Note,
        review_id: None,
        blocks_queue: true,
    }
}

fn review_session_schedule(review: &store::Review) -> BranchSessionSchedule {
    BranchSessionSchedule {
        kind: BranchSessionScheduleKind::Review,
        review_id: Some(review.id.clone()),
        blocks_queue: !review.is_auto,
    }
}

fn commit_session_schedule(kind: BranchSessionScheduleKind) -> BranchSessionSchedule {
    BranchSessionSchedule {
        kind,
        review_id: None,
        blocks_queue: true,
    }
}

fn is_commit_pipeline_session(session: &store::Session) -> bool {
    session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.kind.as_ref())
        .is_some()
}

fn resolve_branch_session_schedule(
    store: &Store,
    branch_id: &str,
    session: &store::Session,
    require_artifact: bool,
) -> Result<Option<BranchSessionSchedule>, String> {
    if is_commit_pipeline_session(session) {
        let commit = store
            .get_commit_by_session(&session.id)
            .map_err(|e| e.to_string())?;
        return match commit {
            Some(commit) if commit.branch_id == branch_id => Ok(Some(commit_session_schedule(
                BranchSessionScheduleKind::CommitPipeline,
            ))),
            Some(_) => Ok(None),
            None if require_artifact => Err(format!(
                "Queued pipeline session {} has no linked commit",
                session.id
            )),
            None => Ok(None),
        };
    }

    if let Some(commit) = store
        .get_commit_by_session(&session.id)
        .map_err(|e| e.to_string())?
    {
        return Ok((commit.branch_id == branch_id)
            .then(|| commit_session_schedule(BranchSessionScheduleKind::Commit)));
    }

    if let Some(note) = store
        .get_note_by_session(&session.id)
        .map_err(|e| e.to_string())?
    {
        return Ok((note.branch_id == branch_id).then(note_session_schedule));
    }

    if let Some(review) = store
        .get_review_by_session(&session.id)
        .map_err(|e| e.to_string())?
    {
        return Ok((review.branch_id == branch_id).then(|| review_session_schedule(&review)));
    }

    if require_artifact {
        Err(format!(
            "Queued session {} has no linked artifact",
            session.id
        ))
    } else {
        Ok(None)
    }
}

fn running_branch_session_kinds(
    store: &Store,
    branch_id: &str,
) -> Result<HashSet<BranchSessionScheduleKind>, String> {
    let running = store.get_running_sessions().map_err(|e| e.to_string())?;
    let mut active = HashSet::new();
    for session in running {
        if let Some(schedule) = resolve_branch_session_schedule(store, branch_id, &session, false)?
        {
            if schedule.blocks_queue {
                active.insert(schedule.kind);
            }
        }
    }
    Ok(active)
}

fn branch_session_launch_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn branch_session_launch_lock_for(branch_id: &str) -> Arc<Mutex<()>> {
    let mut locks = branch_session_launch_locks().lock().unwrap();
    Arc::clone(
        locks
            .entry(branch_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

fn has_queued_user_branch_session(store: &Store, branch_id: &str) -> Result<bool, String> {
    let queued = store
        .get_queued_sessions_for_branch(branch_id)
        .map_err(|e| e.to_string())?;

    for session in queued {
        if let Some(schedule) = resolve_branch_session_schedule(store, branch_id, &session, true)? {
            if schedule.blocks_queue {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

fn branch_session_start_waits_for_provisioning(
    store: &Store,
    branch_id: &str,
) -> Result<bool, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    match branch.branch_type {
        store::BranchType::Local => store
            .get_workdir_for_branch(branch_id)
            .map(|workdir| workdir.is_none())
            .map_err(|e| e.to_string()),
        store::BranchType::Remote => {
            Ok(branch.workspace_status != Some(store::WorkspaceStatus::Running))
        }
    }
}

fn should_queue_branch_session_start(
    store: &Store,
    branch_id: &str,
    session_type: &BranchSessionType,
) -> Result<bool, String> {
    if branch_session_start_waits_for_provisioning(store, branch_id)? {
        return Ok(true);
    }

    if has_queued_user_branch_session(store, branch_id)? {
        return Ok(true);
    }

    let active = running_branch_session_kinds(store, branch_id)?;
    Ok(!can_start_with_active_branch_sessions(
        session_type.schedule_kind(),
        &active,
    ))
}

#[cfg(test)]
fn drainable_session_ids_for_active_set(
    queued: &[(String, BranchSessionSchedule)],
    active: &mut HashSet<BranchSessionScheduleKind>,
) -> Vec<String> {
    let mut drainable = Vec::new();
    for (session_id, schedule) in queued {
        if !can_start_with_active_branch_sessions(schedule.kind, active) {
            break;
        }

        drainable.push(session_id.clone());
        if schedule.blocks_queue {
            active.insert(schedule.kind);
        }
    }
    drainable
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchSessionLaunchContext {
    pub source: String,
    pub scope: String,
    pub commit_sha: String,
    pub review_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BranchSessionLaunchStatus {
    Running,
    Queued,
}

/// Response from starting a branch session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSessionResponse {
    pub session_id: String,
    /// The ID of the artifact created (commit or note).
    pub artifact_id: String,
    pub session_status: BranchSessionLaunchStatus,
}

/// Response from starting a project session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSessionResponse {
    pub session_id: String,
    /// The ID of the project note created for this session.
    pub note_id: String,
}

const PROJECT_SESSION_TIMELINE_REFERENCE_GUIDANCE: &str = "When referring to existing timeline \
items in notes or repo-session instructions, use hashtag references in the form #<type>:<id>, \
for example #note:123, #commit:<sha>, and #review:456. When starting a repo-level session from a \
note, do not paste or rewrite the note contents; reference the note and relevant section instead, \
for example: `Implement \"Step 5: unit tests\" from #note:123`.";

pub(crate) fn build_project_session_action_instructions_with_pikchr_reference(
    is_remote: bool,
    pikchr_grammar_reference: &str,
) -> String {
    let preamble = if is_remote {
        "This top-level project session runs locally and acts as a coordinator. \
For repository-specific execution, use MCP subagent tools.\n\n\
This is a remote-workspace project. Use the project MCP tools to orchestrate work:"
    } else {
        "You have access to the following tools:"
    };

    let start_repo_session_desc = if is_remote {
        "- start_repo_session: Use this to make changes or run tasks in one of the project's \
repositories. It enqueues work and returns a `repo_session_id` immediately. Use \
`expected_outcome=\"note_in_repo\"` for repo notes and `expected_outcome=\"commit\"` for \
code changes/commits; commit sessions create signed-off conventional commits. For remote branches \
this subagent runs on the remote workspace, where file access, notes, and commits must happen.\n\
- wait_for_repo_session: Use this to wait on a previously started repo session by passing the \
`repo_session_id`. It returns the queue state (`queued`, `running`, `completed`, `cancelled`, \
or `failed`), any available artifacts, and activity details. Prefer another \
`wait_for_repo_session` call when the returned activity shows recent progress.\n\
- cancel_repo_session: Use this to abort a queued or running repo session by `repo_session_id` \
when the user wants the session stopped. Cancellation is best used when the user wants to go \
down a different path rather than when you are surprised at how long the session is taking."
    } else {
        "- start_repo_session: Use this to make changes or run tasks in one of the project's \
repositories. It enqueues work and returns a `repo_session_id` immediately. Use \
`expected_outcome=\"note_in_repo\"` for repo notes and `expected_outcome=\"commit\"` for \
code changes/commits; commit sessions create signed-off conventional commits. Do not ask for both \
a note and a commit in a single start_repo_session request — choose one outcome per call. All \
reasoning specific to a repo must be done within a repo session rather than in this project-wide \
context. You MUST NOT write files directly — all file writes MUST go through start_repo_session \
with expected_outcome=\"commit\".\n\
- wait_for_repo_session: Use this to wait on a previously started repo session by passing the \
`repo_session_id`. It returns the queue state (`queued`, `running`, `completed`, `cancelled`, \
or `failed`), any available artifacts, and activity details. Prefer another \
`wait_for_repo_session` call when the returned activity shows recent progress.\n\
- cancel_repo_session: Use this to abort a queued or running repo session by `repo_session_id` \
when the user wants the session stopped. Cancellation is best used when the user wants to go \
down a different path rather than when you are surprised at how long the session is taking."
    };

    let coordinator_reminder = if is_remote {
        "\n\nKeep this project session focused on coordination and synthesis. Do not perform \
repository edits directly here; use `start_repo_session` for implementation work."
    } else {
        ""
    };

    let pikchr_guidance = pikchr_note_guidance(pikchr_grammar_reference);

    format!(
        "The user is requesting work at the project level. Investigate and \
fulfill the request below, then produce a project note summarizing what you found and any \
actions taken.\n\n\
{preamble}\n\n\
{start_repo_session_desc}\n\n\
{PROJECT_SESSION_TIMELINE_REFERENCE_GUIDANCE}\n\n\
{pikchr_guidance}\n\n\
- add_project_repo: Use this when the task requires a repository that isn't yet in the \
project. Pass the GitHub repo slug to add it.\n\n\
IMPORTANT: `add_project_repo` and `start_repo_session` are MCP tools, not shell commands. \
Do not run `which`/`type` for these names and do not ask the user to add repos manually \
unless the MCP tool call itself returns an error. If the tool call fails, report the exact \
error and the next action needed.\
{coordinator_reminder}\n\n\
To discover repositories that might be relevant, use `gh` to explore repos in the user's \
GitHub organizations. Only add repos from organizations the user already belongs to.\n\n\
To return the note, include a horizontal rule (---) followed by the note content. \
Begin the note with a markdown H1 heading as the title.\n\n"
    )
}

/// Start a project-level session.
///
/// Project sessions operate at the project level rather than a specific branch.
/// The agent receives project context (all repos, existing project notes).
/// Sessions receive an MCP server with tools to start repo subagent sessions
/// and add repos.
/// Always creates a ProjectNote stub that is populated when the session completes.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn start_project_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    action_executor: tauri::State<'_, Arc<ActionExecutor>>,
    action_registry: tauri::State<'_, Arc<ActionRegistry>>,
    app_handle: tauri::AppHandle,
    project_id: String,
    prompt: String,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
) -> Result<ProjectSessionResponse, String> {
    let store = get_store(&store)?;

    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    // Build project context for the prompt
    let project_context = build_project_session_context(&store, &project, None);

    let is_remote = project.location == store::ProjectLocation::Remote;
    let pikchr_grammar_reference = resolve_pikchr_grammar_reference(&app_handle, None);
    let action_instructions = build_project_session_action_instructions_with_pikchr_reference(
        is_remote,
        &pikchr_grammar_reference,
    );

    let full_prompt = format!(
        "<action>\n{action_instructions}\n\nProject information:\n{project_context}\n</action>\n\n{prompt}"
    );

    // Resolve working directory — use the project-scoped worktree root (created
    // at project creation time), NOT the repo clone path (~/.staged/repos/…).
    // Project sessions must never have a repos-dir working directory because the
    // agent would see it and start reading/writing files there directly instead
    // of using start_repo_session.
    let working_dir = crate::git::project_worktree_root_for(&project.id)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));

    // Create the session
    let mut session = store::Session::new_running(&full_prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Create a project note stub linked to the session. The frontend uses the
    // backend-resolved sessionStatus to determine whether the note is generating.
    let note = store::ProjectNote::new(&project_id, "", "").with_session(&session.id);
    store
        .create_project_note(&note)
        .map_err(|e| e.to_string())?;
    let note_id = note.id.clone();

    session_runner::start_session(
        SessionConfig {
            session_id: session.id.clone(),
            prompt: full_prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name: None,
            extra_env: vec![],
            mcp_project_id: Some(project_id.clone()),
            action_executor: Some(Arc::clone(&action_executor)),
            action_registry: Some(Arc::clone(&action_registry)),
            remote_working_dir: None,
            image_ids: image_ids.unwrap_or_default(),
            branch_id: None,
            project_id: Some(project_id),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(ProjectSessionResponse {
        session_id: session.id,
        note_id,
    })
}

struct PreparedBranchSessionStart {
    branch: store::Branch,
    session_type: BranchSessionType,
    provider: Option<String>,
    working_dir: PathBuf,
    full_prompt: String,
    pre_head_sha: Option<String>,
    review_tip_sha: Option<String>,
    remote_working_dir: Option<PathBuf>,
}

struct CreatedBranchSession {
    session: store::Session,
    artifact_id: String,
}

fn resolve_branch_session_provider(
    store: &Arc<Store>,
    branch_id: &str,
    session_type: &BranchSessionType,
    provider: Option<String>,
) -> Result<Option<String>, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if matches!(session_type, BranchSessionType::Review) {
        Some(resolve_review_provider(
            provider,
            branch.workspace_name.is_some(),
        ))
        .transpose()
    } else {
        Ok(provider)
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare_branch_session_start(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle,
    branch_id: &str,
    prompt: &str,
    session_type: BranchSessionType,
    provider: Option<String>,
    launch_context: Option<&BranchSessionLaunchContext>,
) -> Result<PreparedBranchSessionStart, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let is_remote = branch.workspace_name.is_some();

    // Resolve working directory and branch context.
    // Remote branches use ws_exec for git operations; local branches use the worktree directly.
    let (working_dir, branch_context, pikchr_grammar_reference) = if is_remote {
        // For remote branches, use the derived clone path as a fallback working dir.
        // The actual work happens via ws_exec, not local filesystem.
        let fallback_dir = resolve_branch_repo_slug(store, &project, &branch)
            .and_then(|repo| crate::paths::repos_dir().map(|d| d.join(repo)))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
        let pikchr_grammar_staging = remote_pikchr_grammar_staging(app_handle, &session_type);
        let base_branch = branch.base_branch.clone();
        let store_for_context = Arc::clone(store);
        let branch_id_for_context = branch_id.to_string();
        let project_id_for_context = branch.project_id.clone();
        let remote_context = tauri::async_runtime::spawn_blocking(move || {
            build_remote_branch_context(
                &workspace_name,
                &base_branch,
                &store_for_context,
                &branch_id_for_context,
                &project_id_for_context,
                pikchr_grammar_staging,
            )
        })
        .await
        .map_err(|e| format!("Failed to build remote branch context: {e}"))?;
        (
            fallback_dir,
            remote_context.branch_context,
            remote_context.pikchr_grammar_reference,
        )
    } else {
        let workdir = store
            .get_workdir_for_branch(branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut worktree_path = PathBuf::from(&workdir.path);
        // Use the project_repo's subpath when the branch is attached to a specific
        // repo (e.g. a secondary repo with no subpath), rather than always falling
        // back to the project-level subpath which may belong to a different repo.
        let effective_subpath = if let Some(repo_id) = branch.project_repo_id.as_deref() {
            store
                .get_project_repo(repo_id)
                .ok()
                .flatten()
                .and_then(|repo| repo.subpath)
        } else {
            project.subpath.clone()
        };
        if let Some(ref subpath) = effective_subpath {
            worktree_path = worktree_path.join(subpath);
        }

        let ctx = build_branch_context(
            &worktree_path,
            &branch.base_branch,
            store,
            branch_id,
            &branch.project_id,
        );
        let pikchr_grammar_reference =
            local_pikchr_grammar_reference_for_session(app_handle, &session_type);
        (worktree_path, ctx, pikchr_grammar_reference)
    };

    let pre_head_sha = if matches!(session_type, BranchSessionType::Commit) {
        if is_remote {
            let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
            match run_blox_blocking(move || {
                blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"])
            })
            .await
            {
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
        }
    } else {
        None
    };

    let review_tip_sha = if matches!(session_type, BranchSessionType::Review) {
        let tip_sha = if is_remote {
            let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
            run_blox_blocking(move || blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"]))
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        } else {
            git::get_head_sha(&working_dir).map_err(|e| format!("Failed to get HEAD SHA: {e}"))?
        };
        Some(tip_sha)
    } else {
        None
    };

    // Build the full prompt with action instructions + project information + branch context.
    let project_information = build_project_context(store, &project, &branch);
    let full_prompt = build_full_prompt_with_pikchr_reference(
        prompt,
        &project_information,
        &branch_context,
        &session_type,
        launch_context,
        Some(&branch.base_branch),
        &pikchr_grammar_reference,
    );

    // Resolve the actual workspace path for remote branches so the remote agent
    // starts in the correct repo directory (not the workspace default).
    let remote_working_dir = if is_remote {
        let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
        let store_for_resolve = Arc::clone(store);
        let branch_for_resolve = branch.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            crate::branches::resolve_branch_workspace_subpath(
                &store_for_resolve,
                &branch_for_resolve,
            )
            .ok()
            .flatten()
            .and_then(|subpath| {
                crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
            })
        })
        .await
        {
            Ok(Some(path)) => Some(PathBuf::from(path)),
            _ => None,
        }
    } else {
        None
    };

    Ok(PreparedBranchSessionStart {
        branch,
        session_type,
        provider,
        working_dir,
        full_prompt,
        pre_head_sha,
        review_tip_sha,
        remote_working_dir,
    })
}

fn insert_running_branch_session(
    store: &Arc<Store>,
    prepared: &PreparedBranchSessionStart,
    prompt: &str,
) -> Result<CreatedBranchSession, String> {
    let mut session = store::Session::new_running(&prepared.full_prompt, &prepared.working_dir);
    if let Some(ref p) = prepared.provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    let artifact_id = match &prepared.session_type {
        BranchSessionType::Note => {
            let note = store::Note::new(&prepared.branch.id, prompt, "").with_session(&session.id);
            store.create_note(&note).map_err(|e| e.to_string())?;
            note.id
        }
        BranchSessionType::Commit => {
            let commit = store::Commit::new_pending(&prepared.branch.id).with_session(&session.id);
            store.create_commit(&commit).map_err(|e| e.to_string())?;
            commit.id
        }
        BranchSessionType::Review => {
            let review = store::Review::new(
                &prepared.branch.id,
                prepared.review_tip_sha.as_deref().unwrap_or("unknown"),
                store::ReviewScope::Branch,
            )
            .with_session(&session.id);
            store.create_review(&review).map_err(|e| e.to_string())?;
            review.id
        }
    };

    Ok(CreatedBranchSession {
        session,
        artifact_id,
    })
}

fn insert_queued_branch_session(
    store: &Arc<Store>,
    branch_id: &str,
    prompt: &str,
    session_type: &BranchSessionType,
    provider: Option<String>,
    image_ids: &[String],
    launch_context: Option<&BranchSessionLaunchContext>,
) -> Result<BranchSessionResponse, String> {
    let queued_prompt = embed_launch_context(prompt, launch_context)?;
    let mut session = store::Session::new_queued(&queued_prompt);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    store
        .set_images_session_id(image_ids, &session.id)
        .map_err(|e| e.to_string())?;

    let artifact_id = match session_type {
        BranchSessionType::Note => {
            let note = store::Note::new(branch_id, prompt, "").with_session(&session.id);
            store.create_note(&note).map_err(|e| e.to_string())?;
            note.id
        }
        BranchSessionType::Commit => {
            let commit = store::Commit::new_pending(branch_id).with_session(&session.id);
            store.create_commit(&commit).map_err(|e| e.to_string())?;
            commit.id
        }
        BranchSessionType::Review => {
            let review = store::Review::new(branch_id, "", store::ReviewScope::Branch)
                .with_session(&session.id);
            store.create_review(&review).map_err(|e| e.to_string())?;
            review.id
        }
    };

    Ok(BranchSessionResponse {
        session_id: session.id,
        artifact_id,
        session_status: BranchSessionLaunchStatus::Queued,
    })
}

#[allow(clippy::too_many_arguments)]
fn launch_running_branch_session(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    prepared: PreparedBranchSessionStart,
    created: CreatedBranchSession,
    image_ids: Vec<String>,
) -> Result<BranchSessionResponse, String> {
    let session_type = prepared.session_type;
    let branch = prepared.branch;
    let session_type_str = session_type.as_str();
    let branch_id = branch.id.clone();
    let project_id = branch.project_id.clone();
    let workspace_name = branch.workspace_name.clone();

    session_runner::emit_session_running(
        &app_handle,
        &created.session.id,
        &branch_id,
        &project_id,
        session_type_str,
    );

    session_runner::start_session(
        SessionConfig {
            session_id: created.session.id.clone(),
            prompt: prepared.full_prompt,
            working_dir: prepared.working_dir,
            agent_session_id: None,
            pre_head_sha: prepared.pre_head_sha,
            provider: prepared.provider,
            workspace_name,
            extra_env: extra_env_for_branch_session(&session_type),
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir: prepared.remote_working_dir,
            image_ids,
            branch_id: Some(branch_id),
            project_id: Some(project_id),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(BranchSessionResponse {
        session_id: created.session.id,
        artifact_id: created.artifact_id,
        session_status: BranchSessionLaunchStatus::Running,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn start_or_queue_branch_session_for_store(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    prompt: String,
    session_type: BranchSessionType,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
    launch_context: Option<BranchSessionLaunchContext>,
) -> Result<BranchSessionResponse, String> {
    let image_ids = image_ids.unwrap_or_default();

    if matches!(
        session_type,
        BranchSessionType::Commit | BranchSessionType::Review
    ) {
        cancel_in_flight_auto_review_for_branch(&store, &registry, &branch_id)?;
    }

    let provider = resolve_branch_session_provider(&store, &branch_id, &session_type, provider)?;
    let launch_lock = branch_session_launch_lock_for(&branch_id);

    {
        let _guard = launch_lock.lock().unwrap();
        if should_queue_branch_session_start(&store, &branch_id, &session_type)? {
            return insert_queued_branch_session(
                &store,
                &branch_id,
                &prompt,
                &session_type,
                provider,
                &image_ids,
                launch_context.as_ref(),
            );
        }
    }

    let prepared = prepare_branch_session_start(
        &store,
        &app_handle,
        &branch_id,
        &prompt,
        session_type.clone(),
        provider.clone(),
        launch_context.as_ref(),
    )
    .await?;

    let created = {
        let _guard = launch_lock.lock().unwrap();
        if should_queue_branch_session_start(&store, &branch_id, &session_type)? {
            return insert_queued_branch_session(
                &store,
                &branch_id,
                &prompt,
                &session_type,
                provider,
                &image_ids,
                launch_context.as_ref(),
            );
        }
        insert_running_branch_session(&store, &prepared, &prompt)?
    };

    launch_running_branch_session(store, registry, app_handle, prepared, created, image_ids)
}

#[allow(clippy::too_many_arguments)]
pub fn queue_branch_session_for_store(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    branch_id: String,
    prompt: String,
    session_type: BranchSessionType,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
    launch_context: Option<BranchSessionLaunchContext>,
) -> Result<BranchSessionResponse, String> {
    let image_ids = image_ids.unwrap_or_default();

    if matches!(
        session_type,
        BranchSessionType::Commit | BranchSessionType::Review
    ) {
        cancel_in_flight_auto_review_for_branch(&store, &registry, &branch_id)?;
    }

    let provider = resolve_branch_session_provider(&store, &branch_id, &session_type, provider)?;
    let launch_lock = branch_session_launch_lock_for(&branch_id);
    let _guard = launch_lock.lock().unwrap();
    insert_queued_branch_session(
        &store,
        &branch_id,
        &prompt,
        &session_type,
        provider,
        &image_ids,
        launch_context.as_ref(),
    )
}

/// Start or queue a branch-scoped session.
///
/// Kept for compatibility with older callers; it now delegates to the backend
/// scheduling guard rather than unconditionally starting work.
#[allow(clippy::too_many_arguments)]
#[tauri::command(rename_all = "camelCase")]
pub async fn start_branch_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    prompt: String,
    session_type: BranchSessionType,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
    launch_context: Option<BranchSessionLaunchContext>,
) -> Result<BranchSessionResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_branch_session_for_store(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        prompt,
        session_type,
        provider,
        image_ids,
        launch_context,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[tauri::command(rename_all = "camelCase")]
pub async fn start_or_queue_branch_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    prompt: String,
    session_type: BranchSessionType,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
    launch_context: Option<BranchSessionLaunchContext>,
) -> Result<BranchSessionResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_branch_session_for_store(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        prompt,
        session_type,
        provider,
        image_ids,
        launch_context,
    )
    .await
}

// =============================================================================
// Queued session commands
// =============================================================================

/// Queue a branch session for later execution.
///
/// Creates a session with `Queued` status and links it to an artifact stub
/// (commit or note), but does NOT resolve working directory, git context,
/// or spawn an agent. The session will be started later via `drain_queued_sessions`.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub fn queue_branch_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    branch_id: String,
    prompt: String,
    session_type: BranchSessionType,
    provider: Option<String>,
    image_ids: Option<Vec<String>>,
    launch_context: Option<BranchSessionLaunchContext>,
) -> Result<BranchSessionResponse, String> {
    let store = get_store(&store)?;
    queue_branch_session_for_store(
        store,
        Arc::clone(&registry),
        branch_id,
        prompt,
        session_type,
        provider,
        image_ids,
        launch_context,
    )
}

/// Drain queued sessions for a branch by starting compatible work.
///
/// Queries queued sessions for the given branch oldest-first, starts compatible
/// note/review pairs, and stops at the first FIFO barrier. Returns whether at
/// least one session was started.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn drain_queued_sessions(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<bool, String> {
    let store = get_store(&store)?;
    drain_queued_sessions_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
    )
    .await
}

/// Start queued branch sessions while they can safely run together.
///
/// This is shared by the Tauri command and backend lifecycle hooks so queue
/// progression remains owned by the backend.
pub async fn drain_queued_sessions_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<bool, String> {
    let queued = store
        .get_queued_sessions_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;

    let mut active = running_branch_session_kinds(&store, &branch_id)?;
    let mut started_any = false;

    for session in queued {
        let schedule = match resolve_branch_session_schedule(&store, &branch_id, &session, true)? {
            Some(schedule) => schedule,
            None => continue,
        };

        if !can_start_with_active_branch_sessions(schedule.kind, &active) {
            break;
        }

        let started = start_queued_session_for_branch(
            Arc::clone(&store),
            Arc::clone(&registry),
            app_handle.clone(),
            branch_id.clone(),
            session,
            schedule.clone(),
            provider.clone(),
        )
        .await?;

        if started {
            started_any = true;
            if schedule.blocks_queue {
                active.insert(schedule.kind);
            }
        } else {
            active = running_branch_session_kinds(&store, &branch_id)?;
        }
    }

    Ok(started_any)
}

#[allow(clippy::too_many_arguments)]
async fn start_queued_session_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    session: store::Session,
    schedule: BranchSessionSchedule,
    provider: Option<String>,
) -> Result<bool, String> {
    if matches!(schedule.kind, BranchSessionScheduleKind::CommitPipeline) {
        return crate::prs::start_queued_commit_pipeline_for_branch(
            store, registry, app_handle, branch_id, session, provider,
        )
        .await;
    }

    let session_type = schedule.kind.branch_session_type().ok_or_else(|| {
        format!(
            "Queued session {} cannot start as an agent session",
            session.id
        )
    })?;

    // Use the original prompt from the queued session.
    let (prompt, launch_context) = extract_launch_context(&session.prompt)?;
    let session_id = session.id.clone();

    // Resolve branch → project (same as start_branch_session).
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let is_remote = branch.workspace_name.is_some();
    let effective_provider = if matches!(session_type, BranchSessionType::Review) {
        let resolved = resolve_review_provider(session.provider.clone().or(provider), is_remote)?;
        if session.provider.as_deref() != Some(resolved.as_str()) {
            store
                .set_session_provider(&session_id, &resolved)
                .map_err(|e| e.to_string())?;
        }
        Some(resolved)
    } else {
        session.provider.clone().or(provider)
    };

    // Resolve working directory and branch context.
    let (working_dir, branch_context, pikchr_grammar_reference) = if is_remote {
        let fallback_dir = resolve_branch_repo_slug(&store, &project, &branch)
            .and_then(|repo| crate::paths::repos_dir().map(|d| d.join(repo)))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
        let pikchr_grammar_staging = remote_pikchr_grammar_staging(&app_handle, &session_type);
        let base_branch = branch.base_branch.clone();
        let store_for_context = Arc::clone(&store);
        let branch_id_for_context = branch_id.clone();
        let project_id_for_context = branch.project_id.clone();
        let remote_context = tauri::async_runtime::spawn_blocking(move || {
            build_remote_branch_context(
                &workspace_name,
                &base_branch,
                &store_for_context,
                &branch_id_for_context,
                &project_id_for_context,
                pikchr_grammar_staging,
            )
        })
        .await
        .map_err(|e| format!("Failed to build remote branch context: {e}"))?;
        (
            fallback_dir,
            remote_context.branch_context,
            remote_context.pikchr_grammar_reference,
        )
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut worktree_path = PathBuf::from(&workdir.path);
        let effective_subpath = if let Some(repo_id) = branch.project_repo_id.as_deref() {
            store
                .get_project_repo(repo_id)
                .ok()
                .flatten()
                .and_then(|repo| repo.subpath)
        } else {
            project.subpath.clone()
        };
        if let Some(ref subpath) = effective_subpath {
            worktree_path = worktree_path.join(subpath);
        }

        let ctx = build_branch_context(
            &worktree_path,
            &branch.base_branch,
            &store,
            &branch_id,
            &branch.project_id,
        );
        let pikchr_grammar_reference =
            local_pikchr_grammar_reference_for_session(&app_handle, &session_type);
        (worktree_path, ctx, pikchr_grammar_reference)
    };

    // Build the full prompt with context.
    let project_information = build_project_context(&store, &project, &branch);
    let full_prompt = build_full_prompt_with_pikchr_reference(
        &prompt,
        &project_information,
        &branch_context,
        &session_type,
        launch_context.as_ref(),
        Some(&branch.base_branch),
        &pikchr_grammar_reference,
    );

    // Atomically transition session from queued to running.
    // If another drain call already claimed this session, bail out.
    let transitioned = store
        .transition_queued_to_running(&session_id)
        .map_err(|e| e.to_string())?;
    if !transitioned {
        return Ok(false);
    }

    store
        .mark_session_artifact_started(&session_id)
        .map_err(|e| e.to_string())?;

    // Update the session's working_dir and prompt now that we have context.
    store
        .prepare_queued_session(&session_id, &working_dir.to_string_lossy(), &full_prompt)
        .map_err(|e| e.to_string())?;

    // Update the review's commit_sha now that we have the working directory.
    // At queue time, reviews are created with an empty commit_sha since the
    // workspace may not exist yet.
    if let Some(ref review_id) = schedule.review_id {
        let tip_sha = if is_remote {
            let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
            run_blox_blocking(move || blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"]))
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        } else {
            git::get_head_sha(&working_dir).map_err(|e| format!("Failed to get HEAD SHA: {e}"))?
        };
        store
            .update_review_commit_sha(review_id, &tip_sha)
            .map_err(|e| e.to_string())?;
    }

    // Compute pre-head SHA for commit sessions.
    let pre_head_sha = match session_type {
        BranchSessionType::Commit => {
            if is_remote {
                let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
                match run_blox_blocking(move || {
                    blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"])
                })
                .await
                {
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
            }
        }
        _ => None,
    };

    // Resolve remote working dir for remote branches.
    let remote_working_dir = if is_remote {
        let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
        let store_for_resolve = Arc::clone(&store);
        let branch_for_resolve = branch.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            crate::branches::resolve_branch_workspace_subpath(
                &store_for_resolve,
                &branch_for_resolve,
            )
            .ok()
            .flatten()
            .and_then(|subpath| {
                crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
            })
        })
        .await
        {
            Ok(Some(path)) => Some(PathBuf::from(path)),
            _ => None,
        }
    } else {
        None
    };

    // Retrieve image IDs linked to this session at queue time.
    let image_ids = store
        .get_image_ids_for_session(&session_id)
        .unwrap_or_default();

    let session_type_str = match session_type {
        BranchSessionType::Commit => "commit",
        BranchSessionType::Note => "note",
        BranchSessionType::Review => "review",
    };

    crate::web_server::emit_to_all(
        &app_handle,
        "session-status-changed",
        session_runner::SessionStatusEvent {
            session_id: session_id.clone(),
            status: "running".to_string(),
            error_message: None,
            completion_reason: None,
            branch_id: Some(branch_id.clone()),
            project_id: Some(branch.project_id.clone()),
            session_type: Some(session_type_str.to_string()),
            is_auto_review: false,
        },
    );

    session_runner::start_session(
        SessionConfig {
            session_id: session_id.clone(),
            prompt: full_prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha,
            provider: effective_provider,
            workspace_name: branch.workspace_name.clone(),
            extra_env: extra_env_for_branch_session(&session_type),
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir,
            image_ids,
            branch_id: Some(branch_id),
            project_id: Some(branch.project_id.clone()),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(true)
}

// =============================================================================
// Auto review commands
// =============================================================================

/// Agents known to be available on remote Blox workstations.
///
/// Must stay in sync with the frontend's `REMOTE_AGENTS` filter in
/// `agent.svelte.ts`.  See the "Why REMOTE_PROVIDER_IDS Exists" note in
/// the branch history for the rationale and future cleanup path.
const REMOTE_PROVIDER_IDS: &[&str] = &["goose", "claude"];

fn available_provider_ids(is_remote: bool) -> Vec<String> {
    if is_remote {
        REMOTE_PROVIDER_IDS.iter().map(|s| s.to_string()).collect()
    } else {
        agent::discover_providers()
            .into_iter()
            .map(|p| p.id)
            .collect()
    }
}

pub(crate) fn read_recent_agent_ids() -> Vec<String> {
    crate::preferences_store_path_buf()
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
        .and_then(|json| {
            json.get("recent-agents")
                .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        })
        .unwrap_or_default()
}

pub(crate) fn select_preferred_provider(
    available_ids: &[String],
    recent_ids: &[String],
) -> Option<String> {
    for agent_id in recent_ids {
        if available_ids.contains(agent_id) {
            return Some(agent_id.clone());
        }
    }

    available_ids.first().cloned()
}

/// Resolve a provider id from an optional explicit selection.
///
/// A non-blank explicit `provider` always wins (after trimming whitespace);
/// blank or whitespace-only values are ignored. When no usable explicit
/// provider is given — the `provider: None` path taken by repo badges, action
/// detection, and any future caller — fall back to the user's preferred
/// available agent via [`select_preferred_provider`]. Returns `None` only when
/// nothing can be resolved at all (no explicit provider and no available
/// agent).
///
/// This is the single shared shape behind every `provider: None` resolution
/// path so the fallback logic can't drift between call sites over time.
pub(crate) fn resolve_preferred_provider_id(
    provider: Option<&str>,
    available_ids: &[String],
    recent_ids: &[String],
) -> Option<String> {
    provider
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| select_preferred_provider(available_ids, recent_ids))
}

/// [`resolve_preferred_provider_id`] sourcing `available_ids` from the installed
/// providers and `recent_ids` from the saved `recent-agents` preference.
///
/// Centralizes the `discover_providers` + `read_recent_agent_ids` scaffolding
/// shared by the badge and action-detection callers so they all discover
/// providers and consult the preference the same way.
pub(crate) fn discover_preferred_provider_id(provider: Option<&str>) -> Option<String> {
    let available_ids: Vec<String> = agent::discover_providers()
        .into_iter()
        .map(|p| p.id)
        .collect();
    resolve_preferred_provider_id(provider, &available_ids, &read_recent_agent_ids())
}

fn missing_review_provider_error(is_remote: bool) -> String {
    if is_remote {
        "No remote ACP provider is configured for review sessions.".to_string()
    } else {
        "No ACP agent found. Install Goose, Claude Code, Codex, Pi, or Amp and ensure it's on your PATH."
            .to_string()
    }
}

fn resolve_provider_from_ids(
    provider: Option<String>,
    available_ids: &[String],
    recent_ids: &[String],
    is_remote: bool,
) -> Result<String, String> {
    if available_ids.is_empty() {
        return Err(missing_review_provider_error(is_remote));
    }

    if let Some(provider) = provider {
        if available_ids.contains(&provider) {
            return Ok(provider);
        }

        let scope = if is_remote { "remote" } else { "local" };
        return Err(format!(
            "Selected agent provider `{provider}` is not available for {scope} review sessions."
        ));
    }

    select_preferred_provider(available_ids, recent_ids)
        .ok_or_else(|| missing_review_provider_error(is_remote))
}

/// Resolve or validate the provider for an agent-backed review.
///
/// When no provider is supplied, mirrors the frontend's `getPreferredAgent`
/// logic: read `recent-agents`, filter against available providers, then fall
/// back to the first available provider.
fn resolve_review_provider(provider: Option<String>, is_remote: bool) -> Result<String, String> {
    resolve_provider_from_ids(
        provider,
        &available_provider_ids(is_remote),
        &read_recent_agent_ids(),
        is_remote,
    )
}

/// Core logic for starting an automatic review for a branch.
///
/// Creates a review with `is_auto = true`, starts a session, and emits
/// `session-status-changed` with `isAutoReview: true` so the frontend
/// can track it.
///
/// When `provider` is `None`, resolves the user's current preferred agent
/// from persisted preferences so that auto-reviews match what the user
/// would get if they clicked "Review" manually.
///
/// This is called both from the Tauri command and from the session runner
/// when a commit session completes.
pub async fn trigger_auto_review(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<BranchSessionResponse, String> {
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

    // Resolve the provider before inserting session/review rows. Auto reviews
    // should only create agent-backed records when the provider is concrete.
    let provider_was_explicit = provider.is_some();
    let provider = resolve_review_provider(provider, is_remote).map_err(|e| {
        log::warn!("[auto_review] no provider available for branch {branch_id}: {e}");
        e
    })?;
    if !provider_was_explicit {
        log::info!("[auto_review] resolved preferred provider: {provider}");
    }

    // Resolve working directory and branch context.
    let (working_dir, branch_context) = if is_remote {
        let fallback_dir = resolve_branch_repo_slug(&store, &project, &branch)
            .and_then(|repo| crate::paths::repos_dir().map(|d| d.join(repo)))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
        let base_branch = branch.base_branch.clone();
        let store_for_context = Arc::clone(&store);
        let branch_id_for_context = branch_id.clone();
        let project_id_for_context = branch.project_id.clone();
        let remote_context = tauri::async_runtime::spawn_blocking(move || {
            build_remote_branch_context(
                &workspace_name,
                &base_branch,
                &store_for_context,
                &branch_id_for_context,
                &project_id_for_context,
                RemotePikchrGrammarStaging::NotNeeded,
            )
        })
        .await
        .map_err(|e| format!("Failed to build remote branch context: {e}"))?;
        (fallback_dir, remote_context.branch_context)
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut worktree_path = PathBuf::from(&workdir.path);
        let effective_subpath = if let Some(repo_id) = branch.project_repo_id.as_deref() {
            store
                .get_project_repo(repo_id)
                .ok()
                .flatten()
                .and_then(|repo| repo.subpath)
        } else {
            project.subpath.clone()
        };
        if let Some(ref subpath) = effective_subpath {
            worktree_path = worktree_path.join(subpath);
        }

        let ctx = build_branch_context(
            &worktree_path,
            &branch.base_branch,
            &store,
            &branch_id,
            &branch.project_id,
        );
        (worktree_path, ctx)
    };

    // Get the current tip SHA for the review anchor
    let tip_sha = if is_remote {
        let workspace_name = branch.workspace_name.as_deref().unwrap().to_string();
        run_blox_blocking(move || blox::ws_exec(&workspace_name, &["git", "rev-parse", "HEAD"]))
            .await
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    } else {
        git::get_head_sha(&working_dir).map_err(|e| format!("Failed to get HEAD SHA: {e}"))?
    };

    // Build the full prompt (reuse Review prompt)
    let prompt = "Review the latest changes on this branch.".to_string();
    let project_information = build_project_context(&store, &project, &branch);
    let full_prompt = build_full_prompt(
        &prompt,
        &project_information,
        &branch_context,
        &BranchSessionType::Review,
        None,
        Some(&branch.base_branch),
    );

    // Create the session
    let session = store::Session::new_running(&full_prompt, &working_dir).with_provider(&provider);
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Create auto review
    let review = store::Review::new(&branch_id, &tip_sha, store::ReviewScope::Branch)
        .with_session(&session.id)
        .with_auto();
    store.create_review(&review).map_err(|e| e.to_string())?;

    // Emit session-status-changed with isAutoReview: true
    crate::web_server::emit_to_all(
        &app_handle,
        "session-status-changed",
        session_runner::SessionStatusEvent {
            session_id: session.id.clone(),
            status: "running".to_string(),
            error_message: None,
            completion_reason: None,
            branch_id: Some(branch_id.clone()),
            project_id: Some(branch.project_id.clone()),
            session_type: Some("review".to_string()),
            is_auto_review: true,
        },
    );

    // Resolve the remote working dir for remote branches
    let remote_working_dir = if is_remote {
        let ws_name = branch.workspace_name.as_deref().unwrap().to_string();
        let store_for_resolve = Arc::clone(&store);
        let branch_for_resolve = branch.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            crate::branches::resolve_branch_workspace_subpath(
                &store_for_resolve,
                &branch_for_resolve,
            )
            .ok()
            .flatten()
            .and_then(|subpath| {
                crate::branches::resolve_workspace_repo_path(&ws_name, &subpath).ok()
            })
        })
        .await
        {
            Ok(Some(path)) => Some(PathBuf::from(path)),
            _ => None,
        }
    } else {
        None
    };

    session_runner::start_session(
        SessionConfig {
            session_id: session.id.clone(),
            prompt: full_prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider: Some(provider),
            workspace_name: branch.workspace_name.clone(),
            extra_env: vec![],
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir,
            image_ids: vec![],
            branch_id: Some(branch_id.clone()),
            project_id: Some(branch.project_id.clone()),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(BranchSessionResponse {
        session_id: session.id,
        artifact_id: review.id,
        session_status: BranchSessionLaunchStatus::Running,
    })
}

/// Resolve the latest git committer timestamp (in milliseconds) for a
/// branch by querying the actual git log.  This covers commits made
/// outside the app that are absent from the `commits` table.
///
/// Returns `0` when the branch has no worktree, no commits, or when the
/// git query fails — callers fall back to the DB-only comparison in that
/// case.
fn latest_git_commit_ms(store: &Arc<Store>, branch_id: &str) -> i64 {
    let branch = match store.get_branch(branch_id) {
        Ok(Some(b)) => b,
        _ => return 0,
    };
    let workdir = match store.get_workdir_for_branch(branch_id) {
        Ok(Some(w)) => w,
        _ => return 0,
    };
    let worktree_path = std::path::Path::new(&workdir.path);
    if !worktree_path.exists() {
        return 0;
    }
    let base_ref = git::origin_ref_for_branch(&branch.base_branch);
    let commits = match git::get_commits_since_base(worktree_path, &base_ref) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    // CommitInfo.timestamp is in seconds; convert to milliseconds.
    commits.iter().map(|c| c.timestamp).max().unwrap_or(0) * 1000
}

pub(crate) fn cancel_in_flight_auto_review_for_branch(
    store: &Arc<Store>,
    registry: &session_runner::SessionRegistry,
    branch_id: &str,
) -> Result<bool, String> {
    let git_ts = latest_git_commit_ms(store, branch_id);
    let Some(review) = store
        .find_fresh_auto_review(branch_id, git_ts)
        .map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };

    let Some(session_id) = review.session_id.as_deref() else {
        return Ok(false);
    };

    let Some(session) = store.get_session(session_id).map_err(|e| e.to_string())? else {
        return Ok(false);
    };

    if !matches!(
        session.status,
        store::SessionStatus::Running | store::SessionStatus::Queued
    ) {
        return Ok(false);
    }

    registry.cancel(session_id);
    let cancelled = store
        .transition_from_active(
            session_id,
            store::SessionStatus::Cancelled,
            None,
            Some(&store::CompletionReason::Interrupted),
        )
        .map_err(|e| e.to_string())?;
    if !cancelled {
        let current = store.get_session(session_id).map_err(|e| e.to_string())?;
        return match current.map(|session| session.status) {
            None | Some(store::SessionStatus::Cancelled) => Ok(true),
            _ => Ok(false),
        };
    }

    Ok(true)
}

/// Find an auto review created after all commits on a branch.
#[tauri::command(rename_all = "camelCase")]
pub async fn find_fresh_auto_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<Option<store::Review>, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        let git_ts = latest_git_commit_ms(&store, &branch_id);
        store
            .find_fresh_auto_review(&branch_id, git_ts)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Update the `is_auto` flag on a review.
#[tauri::command(rename_all = "camelCase")]
pub fn set_review_auto(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    is_auto: bool,
) -> Result<(), String> {
    get_store(&store)?
        .set_review_auto(&review_id, is_auto)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Prompt construction helpers
// =============================================================================

/// Build the branch history context block for a local branch.
pub(crate) fn build_branch_context(
    worktree: &Path,
    base_branch: &str,
    store: &Arc<Store>,
    branch_id: &str,
    project_id: &str,
) -> String {
    let mut parts = vec![context_preamble()];
    let mut timeline: Vec<TimelineEntry> = Vec::new();
    let mut commit_error = None;
    let mut visible_shas: HashSet<String> = HashSet::new();

    // Commits from git log. Always compare against the remote-tracking base;
    // Staged does not keep local base branches fresh.
    let base_ref = git::origin_ref_for_branch(base_branch);
    match git::get_full_commit_log(worktree, &base_ref) {
        Ok(log) if !log.trim().is_empty() => {
            visible_shas = parse_commit_shas(&log);
            timeline.extend(parse_timestamped_log(&log));
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("Failed to get commit log for branch context: {e}");
            commit_error = Some(format!("(Error retrieving commit log: {e})"));
        }
    }

    // Notes and reviews from DB
    let max_commit_ts = timeline.iter().map(|e| e.timestamp).max();
    timeline.extend(note_timeline_entries(store, branch_id, None));
    timeline.extend(review_timeline_entries(
        store,
        branch_id,
        None,
        max_commit_ts,
        &visible_shas,
    ));
    timeline.extend(image_timeline_entries(store, branch_id, None, project_id));

    // Project-level notes
    timeline.extend(project_note_timeline_entries(store, project_id, None));

    parts.push(render_timeline(timeline, commit_error));
    parts.join("\n\n")
}

/// Build the branch history context block for a remote branch.
///
/// Uses `blox ws_exec` to run git commands inside the remote workspace,
/// and reads notes from the DB (which works regardless of worktree location).
struct RemoteBranchContext {
    branch_context: String,
    pikchr_grammar_reference: String,
}

fn build_remote_branch_context(
    workspace_name: &str,
    base_branch: &str,
    store: &Arc<Store>,
    branch_id: &str,
    project_id: &str,
    pikchr_grammar_staging: RemotePikchrGrammarStaging,
) -> RemoteBranchContext {
    let mut parts = vec![context_preamble()];
    let mut timeline: Vec<TimelineEntry> = Vec::new();
    let mut visible_shas: HashSet<String> = HashSet::new();

    // Full commit log via ws_exec.
    // Use merge-base to find the fork point so that only the branch's own
    // commits are included, even after a rebase or when the base ref has
    // moved forward.
    let base_ref = git::origin_ref_for_branch(base_branch);
    let range = if let Ok(mb_output) =
        blox::ws_exec(workspace_name, &["git", "merge-base", &base_ref, "HEAD"])
    {
        let mb = mb_output.trim().to_string();
        format!("{mb}..HEAD")
    } else {
        format!("{base_ref}..HEAD")
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
            visible_shas = parse_commit_shas(&log);
            timeline.extend(parse_timestamped_log(&log));
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("Failed to get remote commit log via ws_exec: {e}");
        }
    }

    // Notes, reviews, images, project notes, and optional Pikchr grammar are
    // written to remote temp files in parallel to reduce ws_exec round trips.
    let max_commit_ts = timeline.iter().map(|e| e.timestamp).max();
    let mut pikchr_grammar_reference = PIKCHR_GRAMMAR_URL.to_string();

    std::thread::scope(|s| {
        let note_handle = s.spawn(|| note_timeline_entries(store, branch_id, Some(workspace_name)));
        let visible_shas = &visible_shas;
        let review_handle = s.spawn(move || {
            review_timeline_entries(
                store,
                branch_id,
                Some(workspace_name),
                max_commit_ts,
                visible_shas,
            )
        });
        let image_handle =
            s.spawn(|| image_timeline_entries(store, branch_id, Some(workspace_name), project_id));
        let project_note_handle =
            s.spawn(|| project_note_timeline_entries(store, project_id, Some(workspace_name)));
        let pikchr_grammar_handle = match &pikchr_grammar_staging {
            RemotePikchrGrammarStaging::Upload { bytes, remote_path } => {
                let remote_path = remote_path.clone();
                Some(s.spawn(move || {
                    upload_pikchr_grammar_to_remote(workspace_name, bytes.as_slice(), remote_path)
                }))
            }
            RemotePikchrGrammarStaging::NotNeeded | RemotePikchrGrammarStaging::FallbackUrl => None,
        };

        match note_handle.join() {
            Ok(entries) => timeline.extend(entries),
            Err(_) => log::error!("note_timeline_entries thread panicked"),
        }
        match review_handle.join() {
            Ok(entries) => timeline.extend(entries),
            Err(_) => log::error!("review_timeline_entries thread panicked"),
        }
        match image_handle.join() {
            Ok(entries) => timeline.extend(entries),
            Err(_) => log::error!("image_timeline_entries thread panicked"),
        }
        match project_note_handle.join() {
            Ok(entries) => timeline.extend(entries),
            Err(_) => log::error!("project_note_timeline_entries thread panicked"),
        }
        if let Some(handle) = pikchr_grammar_handle {
            match handle.join() {
                Ok(reference) => pikchr_grammar_reference = reference,
                Err(_) => log::error!("Pikchr grammar upload thread panicked"),
            }
        }
    });

    parts.push(render_timeline(timeline, None));
    RemoteBranchContext {
        branch_context: parts.join("\n\n"),
        pikchr_grammar_reference,
    }
}

/// Shared preamble for branch context blocks.
fn context_preamble() -> String {
    "This branch represents an ongoing conversation across multiple sessions. \
     Be judicious with your context window, but you are responsible for understanding \
     previous changes or note content from the branch history when they relate to the \
     next step."
        .to_string()
}

fn normalize_subpath(subpath: Option<&str>) -> Option<String> {
    subpath
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn format_repo_label(repo_slug: &str, subpath: Option<&str>) -> String {
    if let Some(subpath) = normalize_subpath(subpath) {
        format!("{repo_slug} (subpath: {subpath})")
    } else {
        repo_slug.to_string()
    }
}

pub(crate) fn build_project_context(
    store: &Arc<Store>,
    project: &store::Project,
    branch: &store::Branch,
) -> String {
    let repos = match store.list_project_repos(&project.id) {
        Ok(repos) => repos,
        Err(e) => {
            log::warn!("Failed to list project repos for prompt context: {e}");
            Vec::new()
        }
    };

    let current_repo_key = if let Some(repo_id) = branch.project_repo_id.as_deref() {
        repos
            .iter()
            .find(|repo| repo.id == repo_id)
            .map(|repo| {
                (
                    repo.github_repo.clone(),
                    normalize_subpath(repo.subpath.as_deref()),
                )
            })
            .or_else(|| {
                store
                    .get_project_repo(repo_id)
                    .ok()
                    .flatten()
                    .map(|repo| (repo.github_repo, normalize_subpath(repo.subpath.as_deref())))
            })
    } else {
        resolve_branch_repo_slug(store, project, branch)
            .map(|repo| (repo, normalize_subpath(project.subpath.as_deref())))
    };

    let current_repo_label = current_repo_key
        .as_ref()
        .map(|(repo, subpath)| format_repo_label(repo, subpath.as_deref()));

    let related_repo_labels = repos
        .iter()
        .filter_map(|repo| {
            let repo_key = (
                repo.github_repo.as_str(),
                normalize_subpath(repo.subpath.as_deref()),
            );
            if let Some((current_repo, current_subpath)) = &current_repo_key {
                if repo_key.0 == current_repo && repo_key.1 == *current_subpath {
                    return None;
                }
            }
            Some(format_repo_label(
                &repo.github_repo,
                repo.subpath.as_deref(),
            ))
        })
        .collect::<Vec<_>>();

    let project_name = project.name.trim();
    let project_name = if project_name.is_empty() {
        "Unnamed Project"
    } else {
        project_name
    };

    let mut lines = vec![format!("You are working in project \"{project_name}\".")];

    if let Some(current_repo_label) = current_repo_label {
        lines.push(format!(
            "This branch is attached to repository `{current_repo_label}`."
        ));
    } else {
        lines.push(
            "This branch is attached to a repository in this project (repo metadata unavailable)."
                .to_string(),
        );
    }

    if related_repo_labels.is_empty() {
        lines
            .push("No additional repositories are currently attached to this project.".to_string());
    } else {
        lines.push("Additional repositories in this project:".to_string());
        for repo_label in related_repo_labels {
            lines.push(format!("- `{repo_label}`"));
        }
    }

    lines.push(
        "You may inspect related repositories for context when relevant. Unless the user explicitly asks for cross-repo changes, only modify files and create commits in this branch's repository.".to_string(),
    );
    lines.join("\n")
}

/// Build the context block for a project-level session.
///
/// Includes: project name, all attached repos (with reasons and per-repo
/// branch timelines), and existing project notes.
pub(crate) fn build_project_session_context(
    store: &Arc<Store>,
    project: &store::Project,
    workspace_name: Option<&str>,
) -> String {
    let project_name = project.name.trim();
    let project_name = if project_name.is_empty() {
        "Unnamed Project"
    } else {
        project_name
    };

    let mut lines = vec![format!("You are working in project \"{project_name}\".")];

    // List all repos
    let repos = store.list_project_repos(&project.id).unwrap_or_default();
    if repos.is_empty() {
        if let Some(ref repo) = project.github_repo {
            lines.push(format!("Primary repository: `{repo}`"));
        } else {
            lines.push("No repositories are attached to this project.".to_string());
        }
    } else {
        lines.push("Repositories in this project:".to_string());
        for repo in &repos {
            let display_repo = repo.head_repo.as_deref().unwrap_or(&repo.github_repo);
            let label = format_repo_label(display_repo, repo.subpath.as_deref());
            let primary_tag = if repo.is_primary { " (primary)" } else { "" };
            let reason_tag = repo
                .reason
                .as_deref()
                .map(|r| format!(" — {r}"))
                .unwrap_or_default();
            lines.push(format!("- `{label}`{primary_tag}{reason_tag}"));
        }
    }

    // Per-repo branch timelines — gives the project-level agent the same
    // awareness of branch activity that branch-level agents receive.
    let all_branches = store
        .list_branches_for_project(&project.id)
        .unwrap_or_default();

    for repo in &repos {
        let repo_branches: Vec<_> = all_branches
            .iter()
            .filter(|b| b.project_repo_id.as_deref() == Some(&repo.id))
            .collect();

        if repo_branches.is_empty() {
            continue;
        }

        let repo_label = format_repo_label(&repo.github_repo, repo.subpath.as_deref());
        lines.push(String::new());
        lines.push(format!("## Repository: {repo_label}"));

        for branch in &repo_branches {
            lines.push(String::new());
            lines.push(format!("### Branch: {}", branch.branch_name));

            let timeline =
                build_branch_timeline_summary(store, branch, branch.workspace_name.as_deref());
            if timeline.is_empty() {
                lines.push("No activity on this branch yet.".to_string());
            } else {
                lines.push(timeline);
            }
        }
    }

    // Also include branches not associated with any repo (legacy or unlinked)
    let unlinked_branches: Vec<_> = all_branches
        .iter()
        .filter(|b| b.project_repo_id.is_none())
        .collect();
    if !unlinked_branches.is_empty() {
        lines.push(String::new());
        lines.push("## Branches (no repo association)".to_string());
        for branch in &unlinked_branches {
            lines.push(String::new());
            lines.push(format!("### Branch: {}", branch.branch_name));

            let timeline =
                build_branch_timeline_summary(store, branch, branch.workspace_name.as_deref());
            if timeline.is_empty() {
                lines.push("No activity on this branch yet.".to_string());
            } else {
                lines.push(timeline);
            }
        }
    }

    // Include existing project notes
    let notes = store.list_project_notes(&project.id).unwrap_or_default();
    let non_empty_notes: Vec<_> = notes.iter().filter(|n| !n.content.is_empty()).collect();
    if !non_empty_notes.is_empty() {
        lines.push(String::new());
        lines.push("## Existing Project Notes".to_string());
        for note in &non_empty_notes {
            let formatted = format_project_note_for_context(
                &note.id,
                &note.title,
                &note.content,
                workspace_name,
            );
            lines.push(formatted);
        }
    }

    lines.join("\n")
}

/// Build a compact timeline summary for a single branch, suitable for
/// inclusion in project-level context.
///
/// Includes commit log (when a local worktree is available), notes, and
/// reviews — but omits project-level notes (those are rendered separately
/// at the project level to avoid duplication).
fn build_branch_timeline_summary(
    store: &Arc<Store>,
    branch: &store::Branch,
    workspace_name: Option<&str>,
) -> String {
    let mut timeline: Vec<TimelineEntry> = Vec::new();
    let mut commit_error = None;
    let mut visible_shas: HashSet<String> = HashSet::new();

    // Attempt to include commit log if we can resolve a local worktree
    if let Ok(Some(workdir)) = store.get_workdir_for_branch(&branch.id) {
        let worktree = std::path::Path::new(&workdir.path);
        if worktree.exists() {
            let base_ref = git::origin_ref_for_branch(&branch.base_branch);
            match git::get_full_commit_log(worktree, &base_ref) {
                Ok(log) if !log.trim().is_empty() => {
                    visible_shas = parse_commit_shas(&log);
                    timeline.extend(parse_timestamped_log(&log));
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!(
                        "Failed to get commit log for branch {} in project context: {e}",
                        branch.branch_name
                    );
                    commit_error = Some(format!("(Error retrieving commit log: {e})"));
                }
            }
        }
    }

    // Notes are written to temp files in the matching execution environment:
    // remote workspace when available, otherwise local temp files.
    let max_commit_ts = timeline.iter().map(|e| e.timestamp).max();
    timeline.extend(note_timeline_entries(store, &branch.id, workspace_name));
    timeline.extend(review_timeline_entries(
        store,
        &branch.id,
        workspace_name,
        max_commit_ts,
        &visible_shas,
    ));
    timeline.extend(image_timeline_entries(
        store,
        &branch.id,
        workspace_name,
        &branch.project_id,
    ));

    if timeline.is_empty() {
        if let Some(err) = commit_error {
            return err;
        }
        return String::new();
    }

    timeline.sort_by_key(|e| (e.timestamp, e.order));

    let mut section = String::new();
    if let Some(err) = commit_error {
        section.push_str(&err);
        section.push('\n');
    }
    for entry in &timeline {
        section.push_str(&entry.content);
        section.push('\n');
    }
    section.trim_end().to_string()
}

// =============================================================================
// Chronological timeline helpers
// =============================================================================

/// A single entry in the branch timeline, sorted by timestamp (Unix seconds).
struct TimelineEntry {
    timestamp: i64,
    /// Position in git's topological order (0 = oldest). Used as a tiebreaker
    /// when multiple commits share the same second-level timestamp.
    order: i64,
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

    timeline.sort_by_key(|e| (e.timestamp, e.order));

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
    // The log is produced with --reverse (oldest-first), so index 0 = oldest.
    let mut order: i64 = 0;
    for record in output.split('\0') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        if let Some((ts_str, display)) = record.split_once('\x01') {
            if let Ok(ts) = ts_str.trim().parse::<i64>() {
                entries.push(TimelineEntry {
                    timestamp: ts,
                    order,
                    content: display.trim().to_string(),
                });
                order += 1;
            }
        }
    }
    entries
}

/// Extract the set of commit SHAs from a timestamped git log.
///
/// The log is produced with `--format=…%x01commit %H%n…`, so each record's
/// display text begins with a `commit <sha>` line. This mirrors the visible-SHA
/// set the branch card builds from its commit list, letting the session context
/// apply the same review-visibility filter (see `review_timeline_entries`).
fn parse_commit_shas(output: &str) -> HashSet<String> {
    let mut shas = HashSet::new();
    for record in output.split('\0') {
        let Some((_, display)) = record.split_once('\x01') else {
            continue;
        };
        let first_line = display.trim_start().lines().next().unwrap_or("");
        if let Some(sha) = first_line.strip_prefix("commit ") {
            let sha = sha.trim();
            if !sha.is_empty() {
                shas.insert(sha.to_string());
            }
        }
    }
    shas
}

/// Write raw bytes to a file inside a remote workspace via `ws_exec`.
///
/// Uses base64 encoding to avoid shell-escaping issues with arbitrary content.
/// For payloads exceeding ~500KB of base64, the data is chunked to stay under
/// ARG_MAX (~1MB on macOS). Returns `Ok(())` on success.
fn write_bytes_to_remote(
    workspace_name: &str,
    bytes: &[u8],
    remote_path: &str,
) -> Result<(), String> {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);

    const CHUNK_SIZE: usize = 500_000;

    if encoded.len() <= CHUNK_SIZE {
        blox::ws_exec(
            workspace_name,
            &[
                "sh",
                "-c",
                &format!("echo '{}' | base64 -d > '{}'", encoded, remote_path),
            ],
        )
        .map_err(|e| format!("Failed to write to remote workspace: {e}"))?;
    } else {
        for (i, chunk) in encoded.as_bytes().chunks(CHUNK_SIZE).enumerate() {
            let chunk_str = std::str::from_utf8(chunk)
                .map_err(|e| format!("Invalid UTF-8 in base64 chunk: {e}"))?;
            let redirect = if i == 0 { ">" } else { ">>" };
            blox::ws_exec(
                workspace_name,
                &[
                    "sh",
                    "-c",
                    &format!(
                        "echo '{}' | base64 -d {} '{}'",
                        chunk_str, redirect, remote_path
                    ),
                ],
            )
            .map_err(|e| format!("Failed to write chunk {i} to remote workspace: {e}"))?;
        }
    }

    Ok(())
}

/// Write note content to a temp file inside a remote workspace via `ws_exec`.
///
/// Uses base64 encoding to avoid shell-escaping issues with arbitrary markdown.
/// Returns the remote path on success, or an error string on failure.
fn write_note_to_remote(
    workspace_name: &str,
    note_id: &str,
    content: &str,
    prefix: &str,
) -> Result<String, String> {
    let remote_path = format!("/tmp/{prefix}-{note_id}.md");
    write_bytes_to_remote(workspace_name, content.as_bytes(), &remote_path)?;
    Ok(remote_path)
}

/// Format a note's content for inclusion in an agent context string.
///
/// When `workspace_name` is `Some`, the note is written to a temp file inside
/// the remote workspace via `ws_exec` and referenced by path. When `None`,
/// the note is written to a local temp file. Both produce the same
/// `See: <path>` output format, keeping large notes out of the prompt.
///
/// Falls back to inlining if the write fails (remote or local).
fn format_note_with_heading(
    id: &str,
    title: &str,
    content: &str,
    workspace_name: Option<&str>,
    heading: &str,
) -> String {
    write_content_to_temp_file(id, content, workspace_name, "staged-note", |path| {
        format!("### {heading}: {title}\n\nSee: `{path}`")
    })
    .unwrap_or_else(|| format!("### {heading}: {title}\n\n{content}"))
}

/// Write content to a temp file (local or remote) and return formatted output via `fmt_ok`.
///
/// Returns `None` if the write fails (caller should fall back to inlining).
fn write_content_to_temp_file(
    id: &str,
    content: &str,
    workspace_name: Option<&str>,
    prefix: &str,
    fmt_ok: impl FnOnce(&str) -> String,
) -> Option<String> {
    if let Some(ws_name) = workspace_name {
        match write_note_to_remote(ws_name, id, content, prefix) {
            Ok(remote_path) => Some(fmt_ok(&remote_path)),
            Err(e) => {
                log::warn!("Failed to write to remote workspace, inlining: {e}");
                None
            }
        }
    } else {
        let path = std::env::temp_dir().join(format!("{prefix}-{id}.md"));
        match std::fs::write(&path, content) {
            Ok(()) => Some(fmt_ok(&path.display().to_string())),
            Err(e) => {
                log::warn!("Failed to write to temp file, inlining: {e}");
                None
            }
        }
    }
}

/// Copy an image to a temp file so the agent can read it with its `Read` tool.
///
/// - **Local** (`workspace_name` is `None`): copies the source file to
///   `/tmp/staged-image-{id}.{ext}`.
/// - **Remote** (`workspace_name` is `Some`): base64-encodes the file and writes
///   it to the remote workspace via `blox ws_exec`, using the same chunking
///   strategy as `write_note_to_remote`.
///
/// Returns the temp path on success, `None` on failure.
fn write_image_to_temp_file(
    source_path: &std::path::Path,
    image_id: &str,
    ext: &str,
    workspace_name: Option<&str>,
) -> Option<String> {
    let temp_filename = format!("staged-image-{image_id}.{ext}");

    if let Some(ws_name) = workspace_name {
        // Remote: read the file and write via the shared helper
        let bytes = match std::fs::read(source_path) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("Failed to read image file for remote transfer: {e}");
                return None;
            }
        };
        let remote_path = format!("/tmp/{temp_filename}");

        if let Err(e) = write_bytes_to_remote(ws_name, &bytes, &remote_path) {
            log::warn!("Failed to write image to remote workspace: {e}");
            return None;
        }

        Some(remote_path)
    } else {
        // Local: clone or copy the file to the system temp directory
        let dest = std::env::temp_dir().join(&temp_filename);

        #[cfg(target_os = "macos")]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;

            extern "C" {
                fn clonefile(
                    src: *const std::ffi::c_char,
                    dst: *const std::ffi::c_char,
                    flags: u32,
                ) -> std::ffi::c_int;
            }

            // Remove any existing file so clonefile doesn't fail with EEXIST.
            // This ensures repeated branch-context builds always get a zero-cost
            // clone rather than falling back to a full byte-for-byte copy.
            let _ = std::fs::remove_file(&dest);

            let src_c = CString::new(source_path.as_os_str().as_bytes()).ok();
            let dst_c = CString::new(dest.as_os_str().as_bytes()).ok();

            let cloned = src_c
                .zip(dst_c)
                .map(|(s, d)| {
                    // SAFETY: both CStrings are valid, null-terminated, and live for the call.
                    unsafe { clonefile(s.as_ptr(), d.as_ptr(), 0) == 0 }
                })
                .unwrap_or(false);

            if !cloned {
                // Falls back to regular copy (cross-volume, non-APFS, etc.)
                if let Err(e) = std::fs::copy(source_path, &dest) {
                    log::warn!("Failed to copy image to temp file: {e}");
                    return None;
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            if let Err(e) = std::fs::copy(source_path, &dest) {
                log::warn!("Failed to copy image to temp file: {e}");
                return None;
            }
        }

        Some(dest.display().to_string())
    }
}

/// Format a single note's content for inclusion in an agent context string.
///
/// Wrapper around `format_note_with_heading` that uses "Note" as the heading
/// and returns `Option<String>` for backward compatibility with timeline filtering.
pub(crate) fn format_note_for_context(
    id: &str,
    title: &str,
    content: &str,
    workspace_name: Option<&str>,
) -> Option<String> {
    Some(format_note_with_heading(
        id,
        title,
        content,
        workspace_name,
        "Note",
    ))
}

/// Convert notes from the DB into timeline entries.
///
/// When `workspace_name` is `Some`, notes are written to temp files inside the
/// remote workspace via `ws_exec`. When `None`, notes are written to local temp
/// files. Both produce path references to keep large notes out of the prompt.
fn note_timeline_entries(
    store: &Arc<Store>,
    branch_id: &str,
    workspace_name: Option<&str>,
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
        if let Some(content) =
            format_note_for_context(&note.id, &note.title, &note.content, workspace_name)
        {
            entries.push(TimelineEntry {
                timestamp: note.completed_at.unwrap_or(note.created_at) / 1000,
                order: 0,
                content,
            });
        }
    }
    entries
}

/// Format a single project note for inclusion in context.
///
/// Wrapper around `format_note_with_heading` that uses "Project Note" as the heading.
fn format_project_note_for_context(
    id: &str,
    title: &str,
    content: &str,
    workspace_name: Option<&str>,
) -> String {
    format_note_with_heading(id, title, content, workspace_name, "Project Note")
}

/// Convert project notes from the DB into timeline entries.
fn project_note_timeline_entries(
    store: &Arc<Store>,
    project_id: &str,
    workspace_name: Option<&str>,
) -> Vec<TimelineEntry> {
    let notes = match store.list_project_notes(project_id) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("Failed to list project notes for branch context: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    for note in &notes {
        if note.content.is_empty() {
            continue; // skip notes still generating
        }
        let content =
            format_project_note_for_context(&note.id, &note.title, &note.content, workspace_name);
        entries.push(TimelineEntry {
            timestamp: note.completed_at.unwrap_or(note.created_at) / 1000,
            order: 0,
            content,
        });
    }
    entries
}

fn should_include_in_history(comment: &store::Comment) -> bool {
    comment.deleted_at.is_none()
        && !matches!(
            comment.comment_type.as_ref(),
            Some(store::CommentType::Information)
        )
}

/// Build the inline comment content for a review (used both for inlining and temp files).
fn format_review_comments(review: &store::Review) -> String {
    let mut content = String::new();

    // Group comments by file path
    let mut by_path: std::collections::BTreeMap<&str, Vec<&store::Comment>> =
        std::collections::BTreeMap::new();
    for comment in review
        .comments
        .iter()
        .filter(|comment| should_include_in_history(comment))
    {
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

    content
}

/// Count (total_comments, issues) for a review's comments.
fn review_summary_counts(review: &store::Review) -> (usize, usize) {
    let total = review
        .comments
        .iter()
        .filter(|comment| should_include_in_history(comment))
        .count();
    let issues = review
        .comments
        .iter()
        .filter(|c| should_include_in_history(c))
        .filter(|c| matches!(c.comment_type.as_ref(), Some(store::CommentType::Issue)))
        .count();
    (total, issues)
}

/// Convert code reviews (with comments) from the DB into timeline entries.
///
/// When `max_commit_ts` is `Some` and a review predates the latest commit,
/// its comments are written to a temp file (like notes) and only a summary
/// line is included in the timeline. Recent reviews are inlined as before.
fn review_timeline_entries(
    store: &Arc<Store>,
    branch_id: &str,
    workspace_name: Option<&str>,
    max_commit_ts: Option<i64>,
    visible_shas: &HashSet<String>,
) -> Vec<TimelineEntry> {
    let reviews = match store.list_reviews_for_branch(branch_id) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Failed to list reviews for branch context: {e}");
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    for review in &reviews {
        if review.is_auto {
            continue;
        }
        // Hide reviews whose originating commit is no longer on the branch,
        // mirroring the branch card timeline so the agent never sees a review
        // the user can't see in the UI.
        if !crate::timeline::review_is_visible_in_timeline(review, |sha| visible_shas.contains(sha))
        {
            continue;
        }
        let has_branch_history_comments = review.comments.iter().any(should_include_in_history);
        if !has_branch_history_comments {
            continue;
        }
        let short_sha = &review.commit_sha[..review.commit_sha.len().min(7)];
        let review_ts_secs = review.completed_at.unwrap_or(review.created_at) / 1000;
        let is_old = max_commit_ts.is_some_and(|ts| review.created_at / 1000 < ts);

        let heading_title = match review.title.as_deref() {
            Some(title) => format!("Code review: {} — {}", title, short_sha),
            None => format!("Code review: {}", short_sha),
        };

        let content = if is_old {
            let (total, issues) = review_summary_counts(review);
            let comment_detail = format_review_comments(review);
            let full_content = format!("### {heading_title}\n{comment_detail}");

            let summary_suffix = format!("{total} comments, {issues} issues");

            write_content_to_temp_file(
                &review.id,
                &full_content,
                workspace_name,
                "staged-review",
                |path| format!("### {heading_title} — {summary_suffix}\n\nSee: `{path}`"),
            )
            .unwrap_or_else(|| {
                // Fallback: inline if file write fails
                format!("### {heading_title}\n{comment_detail}")
            })
        } else {
            let comment_detail = format_review_comments(review);
            format!("### {heading_title}\n{comment_detail}")
        };

        entries.push(TimelineEntry {
            timestamp: review_ts_secs,
            order: 0,
            content,
        });
    }
    entries
}

/// Convert images from the DB into timeline entries.
///
/// When possible, each image is copied to a temp file (local or remote) and
/// referenced by path so the agent can `Read` it.  Falls back to a text-only
/// placeholder if the file cannot be written.
fn image_timeline_entries(
    store: &Arc<Store>,
    branch_id: &str,
    workspace_name: Option<&str>,
    project_id: &str,
) -> Vec<TimelineEntry> {
    let images = match store.list_images_for_branch(branch_id) {
        Ok(imgs) => imgs,
        Err(e) => {
            log::warn!("Failed to list images for branch context: {e}");
            return Vec::new();
        }
    };

    images
        .iter()
        .map(|img| {
            let size_label = if img.size_bytes > 1_000_000 {
                format!("{:.1} MB", img.size_bytes as f64 / 1_000_000.0)
            } else if img.size_bytes > 1_000 {
                format!("{:.0} KB", img.size_bytes as f64 / 1_000.0)
            } else {
                format!("{} B", img.size_bytes)
            };

            // Try to resolve the source path and copy to a temp file
            let content = match store::images::image_file_path(project_id, &img.id, &img.filename) {
                Ok(source_path) if source_path.exists() => {
                    // Extension is always present since image_file_path constructs the
                    // path from img.filename which includes an extension. The "bin"
                    // fallback is defensive but should be unreachable in practice.
                    let ext = source_path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("bin");
                    match write_image_to_temp_file(&source_path, &img.id, ext, workspace_name) {
                        Some(temp_path) => format!(
                            "### Image: {}\n\nSee: `{}`",
                            img.filename, temp_path
                        ),
                        None => format!(
                            "### Image: {}\n\nAttached image ({}, {}). If this image was included in the current prompt, it will appear as an image content block.",
                            img.filename, img.mime_type, size_label
                        ),
                    }
                }
                _ => format!(
                    "### Image: {}\n\nAttached image ({}, {}). If this image was included in the current prompt, it will appear as an image content block.",
                    img.filename, img.mime_type, size_label
                ),
            };

            TimelineEntry {
                timestamp: img.created_at / 1000,
                order: 0,
                content,
            }
        })
        .collect()
}

fn shell_quote_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Assemble the full prompt from action instructions + branch context + user prompt.
pub(crate) fn build_full_prompt(
    user_prompt: &str,
    project_information: &str,
    branch_context: &str,
    session_type: &BranchSessionType,
    launch_context: Option<&BranchSessionLaunchContext>,
    base_branch: Option<&str>,
) -> String {
    build_full_prompt_with_pikchr_reference(
        user_prompt,
        project_information,
        branch_context,
        session_type,
        launch_context,
        base_branch,
        PIKCHR_GRAMMAR_URL,
    )
}

pub(crate) fn build_full_prompt_with_pikchr_reference(
    user_prompt: &str,
    project_information: &str,
    branch_context: &str,
    session_type: &BranchSessionType,
    launch_context: Option<&BranchSessionLaunchContext>,
    base_branch: Option<&str>,
    pikchr_grammar_reference: &str,
) -> String {
    let mut action_instructions = match session_type {
        BranchSessionType::Note => {
            "The user is requesting a note. Generate a note based on their prompt below.

You may use any tools needed to research and gather information, but do NOT create \
any commits.

To return the note, your final response must include the structure shown below. \
Before the `---` separator, emit a `suggested-next-steps` fenced block that suggests \
what the user might want to do next. The block must contain a single JSON object with \
two nullable string fields:

```suggested-next-steps
{\"suggestedNextCommitStep\": \"Fix the null pointer bug in parser.rs\", \"suggestedNextNoteStep\": \"Make a plan to fix the null pointer bug\"}
```

Guidelines for suggested next steps:
- Keep suggestions very concise (a few words). They are shown alongside the note title, \
so you can assume the user has already read the title for context. \
Do NOT repeat information from the title.
- If the note is a plan, suggest a commit to implement it: \
{\"suggestedNextCommitStep\": \"Implement this plan\", \"suggestedNextNoteStep\": null}
- If the note is a plan with multiple options, pick the best option: \
{\"suggestedNextCommitStep\": \"Implement option 2: use Redis cache\", \"suggestedNextNoteStep\": null}
- If the note is bug research, suggest both a fix and a deeper plan: \
{\"suggestedNextCommitStep\": \"Fix this bug\", \"suggestedNextNoteStep\": \"Plan a fix for this bug\"}
- If the note is pure research or informational with no clear next action: \
{\"suggestedNextCommitStep\": null, \"suggestedNextNoteStep\": null}

Then, after the suggested-next-steps block, include the note itself:

---
# <Title>
<Body>

Formatting requirements:
- The opening fence line for suggested-next-steps must be exactly: ```suggested-next-steps
- The closing fence line must be exactly: ```
- Put only the JSON object inside the suggested-next-steps block (no prose or markdown).
- Do not wrap the block in any additional code fences.
- `---` must be on its own line, with a newline immediately before and after it.
- The note content must start immediately after `---` with a markdown H1 (`# Title`).
- Do not wrap the note in code fences.".to_string()
        }
        BranchSessionType::Commit => {
            "The user is requesting you make a commit based on the prompt below. Make the necessary \
code changes, following any verification or formatting steps as instructed, and then \
create a commit with a conventional commit message. This commit should describe what \
was requested and how it was fulfilled.

Before creating the commit:
- Use the user's global git identity (`git config --global user.name` and `git config --global user.email`) \
for both the author and committer. Do not use placeholder identities.
- Create the commit with a DCO signoff (`git commit --signoff`) so the commit message includes a \
matching `Signed-off-by` trailer.".to_string()
        }
        BranchSessionType::Review => {
            let base_ref = git::origin_ref_for_branch(base_branch.unwrap_or("main"));
            let quoted_base_ref = shell_quote_arg(&base_ref);
            format!(
                "The user is requesting an AI code review of the current branch.\n\
\n\
Review the code changes on this branch by running a diff from the remote-tracking base ref: \
`git diff $(git merge-base {quoted_base_ref} HEAD)..HEAD`. Do not compare against the \
local base branch, which may be stale.\n\
\n\
Do NOT create any commits or modify any files.\n\
\n\
## Review philosophy\n\
\n\
Your comments should tell the story of the change — focus on the \"why\", potential issues, \
and non-obvious implications. Do NOT exhaustively document every line or restate what the code \
obviously does. It's fine to have no comments for trivial or self-explanatory files. \
Aim for quality over quantity: a few insightful comments are better than many shallow ones.\n\
\n\
## Comment types\n\
\n\
Each comment MUST include a `type` field. Choose the type carefully:\n\
\n\
- `\"information\"` — Contextual explanation, \"why\" behind a change, or architectural observation. \
Use this for comments that help a reader understand the change but don't require action. \
These are shown as subtle hold-to-reveal annotations, not inline comments. \
Examples: explaining a non-obvious design decision, noting how a change fits into the broader architecture, \
describing what the old code was doing and why it changed.\n\
\n\
- `\"suggestion\"` — A recommended improvement that isn't strictly necessary. \
The code works but could be better. \
Examples: a more idiomatic approach, better naming, a simplification.\n\
\n\
- `\"warning\"` — A potential issue or concern that deserves attention. \
Not a definite bug, but something that could cause problems. \
Examples: missing edge case handling, potential performance issue, fragile assumption.\n\
\n\
- `\"issue\"` — A bug or correctness problem that should be fixed. \
Examples: off-by-one error, null pointer risk, logic error, security vulnerability.\n\
\n\
Most comments in a typical review should be `\"information\"` or `\"suggestion\"`. \
Reserve `\"warning\"` and `\"issue\"` for genuine concerns.\n\
\n\
## Output format\n\
\n\
Your response must start directly with the review-title fenced block below — \
do not output any preamble, commentary, or thinking before it.\n\
\n\
Provide a single-sentence title (max 15 words) that conveys your overall \
confidence level in the changes. Do not describe what the changes do — instead focus on \
how confident you are that they are correct and safe. Wrap it in a fenced block:\n\
\n\
```review-title\n\
Looks solid overall with one minor edge case worth checking\n\
```\n\
\n\
Then return your review comments as exactly one fenced JSON block:\n\
\n\
```review-comments\n\
[\n\
  {{\n\
    \"path\": \"src/foo.ts\",\n\
    \"span\": {{ \"start\": 10, \"end\": 15 }},\n\
    \"type\": \"information\",\n\
    \"content\": \"This refactors the error handling from panicking to returning Results, which aligns with the broader error-handling migration across the codebase.\"\n\
  }},\n\
  {{\n\
    \"path\": \"src/bar.rs\",\n\
    \"span\": {{ \"start\": 42, \"end\": 45 }},\n\
    \"type\": \"warning\",\n\
    \"content\": \"This unwrap() could panic if the connection pool is exhausted under load.\"\n\
  }}\n\
]\n\
```\n\
\n\
Formatting requirements:\n\
- The opening fence line for the title must be exactly: ```review-title\n\
- The opening fence line for comments must be exactly: ```review-comments\n\
- Each closing fence line must be exactly: ```\n\
- Put only plain text (no markdown) inside the review-title block.\n\
- Put only the JSON array inside the review-comments block (no prose or markdown).\n\
- Do not wrap these blocks in any additional code fences.\n\
\n\
Rules:\n\
- `span` uses 0-indexed line numbers from the \"after\" side of the diff (exclusive end).\n\
- Only comment on changed files.\n\
- Be specific and actionable — reference the actual code, not generic advice.")
        }
    };

    if matches!(session_type, BranchSessionType::Note) {
        action_instructions.push_str("\n\n");
        action_instructions.push_str(&pikchr_note_guidance(pikchr_grammar_reference));
    }

    let action_tag = format!(
        "<action>\n{action_instructions}\n\nProject information:\n{project_information}\n</action>"
    );
    let branch_history = render_branch_history(branch_context, launch_context);

    format!(
        "{action_tag}\n\n\
         <branch-history>\n\
         {branch_history}\n\
         </branch-history>\n\n\
         {user_prompt}"
    )
}

fn render_branch_history(
    branch_context: &str,
    launch_context: Option<&BranchSessionLaunchContext>,
) -> String {
    let mut parts = Vec::new();
    if !branch_context.trim().is_empty() {
        parts.push(branch_context.trim_end().to_string());
    }
    if let Some(entry) = render_launch_context_entry(launch_context) {
        parts.push(entry);
    }
    parts.join("\n\n")
}

fn render_launch_context_entry(
    launch_context: Option<&BranchSessionLaunchContext>,
) -> Option<String> {
    let context = launch_context?;
    if context.source != "diff_viewer" {
        return None;
    }

    let scope_suffix = match context.scope.as_str() {
        "branch" => String::new(),
        _ => format!(" (scope: {})", context.scope),
    };

    let mut entry = format!(
        "Viewed diff before starting this session: commit {}{}.",
        context.commit_sha, scope_suffix
    );
    if let Some(review_id) = context.review_id.as_deref() {
        entry = format!(
            "Viewed diff before starting this session: review {} on commit {}{}.",
            review_id, context.commit_sha, scope_suffix
        );
    }
    Some(entry)
}

pub(crate) fn embed_launch_context(
    prompt: &str,
    launch_context: Option<&BranchSessionLaunchContext>,
) -> Result<String, String> {
    let Some(context) = launch_context else {
        return Ok(prompt.to_string());
    };
    let json = serde_json::to_string(context).map_err(|e| e.to_string())?;
    Ok(format!(
        "<launch-context>{json}</launch-context>\n\n{}",
        prompt.trim_start()
    ))
}

pub(crate) fn extract_launch_context(
    prompt: &str,
) -> Result<(String, Option<BranchSessionLaunchContext>), String> {
    const OPEN: &str = "<launch-context>";
    const CLOSE: &str = "</launch-context>";

    let Some(rest) = prompt.strip_prefix(OPEN) else {
        return Ok((prompt.to_string(), None));
    };
    let Some((json, remainder)) = rest.split_once(CLOSE) else {
        return Err("Queued session prompt had malformed launch context".to_string());
    };
    let context =
        serde_json::from_str::<BranchSessionLaunchContext>(json).map_err(|e| e.to_string())?;
    Ok((remainder.trim_start().to_string(), Some(context)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn setup_branch_store() -> (Arc<Store>, store::Branch) {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        (store, branch)
    }

    fn setup_branch_store_with_workdir() -> (Arc<Store>, store::Branch) {
        let (store, branch) = setup_branch_store();
        let workdir_path = format!("/tmp/staged-test-workdir-{}", branch.id);
        let workdir =
            store::Workdir::new(&branch.project_id, &workdir_path).with_branch(&branch.id);
        store.create_workdir(&workdir).unwrap();
        (store, branch)
    }

    fn setup_remote_branch_store(status: store::WorkspaceStatus) -> (Arc<Store>, store::Branch) {
        let store = Arc::new(Store::in_memory().unwrap());
        let mut project = store::Project::new("test-owner/test-repo");
        project.location = store::ProjectLocation::Remote;
        store.create_project(&project).unwrap();
        let mut branch =
            store::Branch::new_remote(&project.id, "feature", "main", "test-workspace");
        branch.workspace_status = Some(status);
        store.create_branch(&branch).unwrap();
        (store, branch)
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn create_auto_review(
        store: &Arc<Store>,
        branch_id: &str,
        status: store::SessionStatus,
    ) -> (store::Session, store::Review) {
        let session = match status {
            store::SessionStatus::Running => {
                store::Session::new_running("auto review", Path::new("/tmp"))
            }
            store::SessionStatus::Queued => store::Session::new_queued("auto review"),
            store::SessionStatus::Completed => {
                store::Session::new_running("auto review", Path::new("/tmp"))
            }
            other => panic!("unsupported auto review test status: {}", other.as_str()),
        };
        store.create_session(&session).unwrap();
        if status != store::SessionStatus::Running && status != store::SessionStatus::Queued {
            store
                .update_session_status(&session.id, status, None, None)
                .unwrap();
        }

        let review = store::Review::new(branch_id, "abc123", store::ReviewScope::Branch)
            .with_session(&session.id)
            .with_auto();
        store.create_review(&review).unwrap();
        (session, review)
    }

    fn create_session_with_status(
        store: &Arc<Store>,
        prompt: &str,
        status: store::SessionStatus,
    ) -> store::Session {
        let session = match status {
            store::SessionStatus::Queued => store::Session::new_queued(prompt),
            store::SessionStatus::Running => store::Session::new_running(prompt, Path::new("/tmp")),
            other => panic!("unsupported scheduler test status: {}", other.as_str()),
        };
        store.create_session(&session).unwrap();
        session
    }

    fn create_branch_note_session(
        store: &Arc<Store>,
        branch_id: &str,
        status: store::SessionStatus,
    ) -> store::Session {
        let session = create_session_with_status(store, "note", status);
        let note = store::Note::new(branch_id, "note", "").with_session(&session.id);
        store.create_note(&note).unwrap();
        session
    }

    fn create_branch_review_session(
        store: &Arc<Store>,
        branch_id: &str,
        status: store::SessionStatus,
    ) -> store::Session {
        let session = create_session_with_status(store, "review", status);
        let review = store::Review::new(branch_id, "abc123", store::ReviewScope::Branch)
            .with_session(&session.id);
        store.create_review(&review).unwrap();
        session
    }

    fn create_branch_commit_session(
        store: &Arc<Store>,
        branch_id: &str,
        status: store::SessionStatus,
    ) -> store::Session {
        let session = create_session_with_status(store, "commit", status);
        let commit = store::Commit::new_pending(branch_id).with_session(&session.id);
        store.create_commit(&commit).unwrap();
        session
    }

    fn create_branch_commit_pipeline_session(
        store: &Arc<Store>,
        branch_id: &str,
    ) -> store::Session {
        let mut session = store::Session::new_running("rebase", Path::new("/tmp"));
        session.pipeline =
            Some(store::PipelineExecution::from_steps(&[]).with_kind(store::PipelineKind::Rebase));
        store.create_session(&session).unwrap();
        let commit = store::Commit::new_pending(branch_id).with_session(&session.id);
        store.create_commit(&commit).unwrap();
        session
    }

    fn schedule(kind: BranchSessionScheduleKind) -> BranchSessionSchedule {
        BranchSessionSchedule {
            kind,
            review_id: None,
            blocks_queue: true,
        }
    }

    #[test]
    fn running_note_allows_queued_review() {
        let (store, branch) = setup_branch_store();
        create_branch_note_session(&store, &branch.id, store::SessionStatus::Running);

        let active = running_branch_session_kinds(&store, &branch.id).unwrap();

        assert!(can_start_with_active_branch_sessions(
            BranchSessionScheduleKind::Review,
            &active
        ));
    }

    #[test]
    fn running_note_allows_queued_note() {
        let (store, branch) = setup_branch_store();
        create_branch_note_session(&store, &branch.id, store::SessionStatus::Running);

        let active = running_branch_session_kinds(&store, &branch.id).unwrap();

        assert!(can_start_with_active_branch_sessions(
            BranchSessionScheduleKind::Note,
            &active
        ));
    }

    #[test]
    fn running_review_allows_queued_note() {
        let (store, branch) = setup_branch_store();
        create_branch_review_session(&store, &branch.id, store::SessionStatus::Running);

        let active = running_branch_session_kinds(&store, &branch.id).unwrap();

        assert!(can_start_with_active_branch_sessions(
            BranchSessionScheduleKind::Note,
            &active
        ));
    }

    #[test]
    fn running_review_blocks_queued_review() {
        let (store, branch) = setup_branch_store();
        create_branch_review_session(&store, &branch.id, store::SessionStatus::Running);

        let active = running_branch_session_kinds(&store, &branch.id).unwrap();

        assert!(!can_start_with_active_branch_sessions(
            BranchSessionScheduleKind::Review,
            &active
        ));
    }

    #[test]
    fn running_commit_blocks_queued_note_and_review() {
        let (store, branch) = setup_branch_store();
        create_branch_commit_session(&store, &branch.id, store::SessionStatus::Running);

        let active = running_branch_session_kinds(&store, &branch.id).unwrap();

        assert!(!can_start_with_active_branch_sessions(
            BranchSessionScheduleKind::Note,
            &active
        ));
        assert!(!can_start_with_active_branch_sessions(
            BranchSessionScheduleKind::Review,
            &active
        ));
    }

    #[test]
    fn branch_start_decision_queues_local_branch_without_workdir() {
        let (store, branch) = setup_branch_store();

        for session_type in [
            BranchSessionType::Note,
            BranchSessionType::Review,
            BranchSessionType::Commit,
        ] {
            assert!(should_queue_branch_session_start(&store, &branch.id, &session_type).unwrap());
        }
    }

    #[test]
    fn branch_start_decision_queues_remote_branch_until_workspace_running() {
        let (store, branch) = setup_remote_branch_store(store::WorkspaceStatus::Starting);

        for session_type in [
            BranchSessionType::Note,
            BranchSessionType::Review,
            BranchSessionType::Commit,
        ] {
            assert!(should_queue_branch_session_start(&store, &branch.id, &session_type).unwrap());
        }
    }

    #[test]
    fn branch_start_decision_uses_compatibility_for_running_remote_branch() {
        let (store, branch) = setup_remote_branch_store(store::WorkspaceStatus::Running);
        create_branch_note_session(&store, &branch.id, store::SessionStatus::Running);

        assert!(
            !should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Review)
                .unwrap()
        );
        assert!(
            !should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Note)
                .unwrap()
        );
        assert!(
            should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Commit)
                .unwrap()
        );
    }

    #[test]
    fn branch_start_decision_queues_all_user_modes_behind_running_commit() {
        let (store, branch) = setup_branch_store_with_workdir();
        create_branch_commit_session(&store, &branch.id, store::SessionStatus::Running);

        for session_type in [
            BranchSessionType::Note,
            BranchSessionType::Review,
            BranchSessionType::Commit,
        ] {
            assert!(should_queue_branch_session_start(&store, &branch.id, &session_type).unwrap());
        }
    }

    #[test]
    fn branch_start_decision_allows_parallel_notes_and_note_review_overlap() {
        let (store, branch) = setup_branch_store_with_workdir();
        create_branch_note_session(&store, &branch.id, store::SessionStatus::Running);

        assert!(
            !should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Review)
                .unwrap()
        );
        assert!(
            !should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Note)
                .unwrap()
        );
        assert!(
            should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Commit)
                .unwrap()
        );

        let (store, branch) = setup_branch_store_with_workdir();
        create_branch_review_session(&store, &branch.id, store::SessionStatus::Running);

        assert!(
            !should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Note)
                .unwrap()
        );
        assert!(
            should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Review)
                .unwrap()
        );
        assert!(
            should_queue_branch_session_start(&store, &branch.id, &BranchSessionType::Commit)
                .unwrap()
        );
    }

    #[test]
    fn branch_start_decision_queues_behind_existing_queued_user_session() {
        let (store, branch) = setup_branch_store_with_workdir();
        create_branch_note_session(&store, &branch.id, store::SessionStatus::Queued);

        for session_type in [
            BranchSessionType::Note,
            BranchSessionType::Review,
            BranchSessionType::Commit,
        ] {
            assert!(should_queue_branch_session_start(&store, &branch.id, &session_type).unwrap());
        }
    }

    #[test]
    fn running_commit_pipeline_blocks_queued_note_and_review() {
        let (store, branch) = setup_branch_store();
        create_branch_commit_pipeline_session(&store, &branch.id);

        let active = running_branch_session_kinds(&store, &branch.id).unwrap();

        assert!(!can_start_with_active_branch_sessions(
            BranchSessionScheduleKind::Note,
            &active
        ));
        assert!(!can_start_with_active_branch_sessions(
            BranchSessionScheduleKind::Review,
            &active
        ));
    }

    #[test]
    fn queued_commit_acts_as_fifo_barrier() {
        let mut active = HashSet::new();
        let queued = vec![
            (
                "note-1".to_string(),
                schedule(BranchSessionScheduleKind::Note),
            ),
            (
                "commit".to_string(),
                schedule(BranchSessionScheduleKind::Commit),
            ),
            (
                "review-1".to_string(),
                schedule(BranchSessionScheduleKind::Review),
            ),
        ];

        let drainable = drainable_session_ids_for_active_set(&queued, &mut active);

        assert_eq!(drainable, vec!["note-1".to_string()]);
    }

    #[test]
    fn drain_scan_starts_multiple_queued_notes_before_commit_barrier() {
        let mut active = HashSet::new();
        let queued = vec![
            (
                "note-1".to_string(),
                schedule(BranchSessionScheduleKind::Note),
            ),
            (
                "note-2".to_string(),
                schedule(BranchSessionScheduleKind::Note),
            ),
            (
                "commit".to_string(),
                schedule(BranchSessionScheduleKind::Commit),
            ),
            (
                "note-3".to_string(),
                schedule(BranchSessionScheduleKind::Note),
            ),
        ];

        let drainable = drainable_session_ids_for_active_set(&queued, &mut active);

        assert_eq!(drainable, vec!["note-1".to_string(), "note-2".to_string()]);
    }

    #[test]
    fn running_auto_review_does_not_block_queued_user_sessions() {
        let (store, branch) = setup_branch_store();
        create_auto_review(&store, &branch.id, store::SessionStatus::Running);

        let active = running_branch_session_kinds(&store, &branch.id).unwrap();

        assert!(active.is_empty());
        for kind in [
            BranchSessionScheduleKind::Note,
            BranchSessionScheduleKind::Review,
            BranchSessionScheduleKind::Commit,
        ] {
            assert!(can_start_with_active_branch_sessions(kind, &active));
        }
    }

    #[test]
    fn branch_start_decision_ignores_auto_review_barriers() {
        let (store, branch) = setup_branch_store_with_workdir();
        create_auto_review(&store, &branch.id, store::SessionStatus::Running);
        create_auto_review(&store, &branch.id, store::SessionStatus::Queued);

        for session_type in [
            BranchSessionType::Note,
            BranchSessionType::Review,
            BranchSessionType::Commit,
        ] {
            assert!(!should_queue_branch_session_start(&store, &branch.id, &session_type).unwrap());
        }
    }

    #[test]
    fn explicit_queue_response_reports_queued_status() {
        let (store, branch) = setup_branch_store();
        let registry = Arc::new(session_runner::SessionRegistry::new());

        let response = queue_branch_session_for_store(
            Arc::clone(&store),
            registry,
            branch.id.clone(),
            "capture a note".to_string(),
            BranchSessionType::Note,
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(response.session_status, BranchSessionLaunchStatus::Queued);
        let session = store.get_session(&response.session_id).unwrap().unwrap();
        assert_eq!(session.status, store::SessionStatus::Queued);
    }

    #[test]
    fn drain_scan_starts_compatible_oldest_sessions_and_stops_at_incompatible_session() {
        let mut active = HashSet::new();
        let queued = vec![
            (
                "note-1".to_string(),
                schedule(BranchSessionScheduleKind::Note),
            ),
            (
                "review-1".to_string(),
                schedule(BranchSessionScheduleKind::Review),
            ),
            (
                "note-2".to_string(),
                schedule(BranchSessionScheduleKind::Note),
            ),
            (
                "commit".to_string(),
                schedule(BranchSessionScheduleKind::Commit),
            ),
            (
                "note-3".to_string(),
                schedule(BranchSessionScheduleKind::Note),
            ),
        ];

        let drainable = drainable_session_ids_for_active_set(&queued, &mut active);

        assert_eq!(
            drainable,
            vec![
                "note-1".to_string(),
                "review-1".to_string(),
                "note-2".to_string()
            ]
        );
    }

    #[test]
    fn drain_scan_does_not_skip_over_manual_review_blocker() {
        let mut active = HashSet::new();
        let queued = vec![
            (
                "review-1".to_string(),
                schedule(BranchSessionScheduleKind::Review),
            ),
            (
                "review-2".to_string(),
                schedule(BranchSessionScheduleKind::Review),
            ),
            (
                "note-1".to_string(),
                schedule(BranchSessionScheduleKind::Note),
            ),
        ];

        let drainable = drainable_session_ids_for_active_set(&queued, &mut active);

        assert_eq!(drainable, vec!["review-1".to_string()]);
    }

    fn create_branch_review(
        store: &Arc<Store>,
        branch_id: &str,
        commit_sha: &str,
    ) -> store::Review {
        let review = store::Review::new(branch_id, commit_sha, store::ReviewScope::Branch);
        store.create_review(&review).unwrap();
        review
    }

    fn add_agent_comment(
        store: &Arc<Store>,
        review_id: &str,
        content: &str,
        comment_type: store::CommentType,
    ) -> store::Comment {
        let comment = store::Comment::new("src/lib.rs", crate::git::Span::new(10, 10), content)
            .with_author(store::CommentAuthor::Agent)
            .with_comment_type(comment_type);
        store.add_comment(review_id, &comment).unwrap();
        comment
    }

    #[test]
    fn select_preferred_provider_uses_first_recent_available_provider() {
        let available = ids(&["goose", "claude"]);
        let recent = ids(&["codex", "claude", "goose"]);

        assert_eq!(
            select_preferred_provider(&available, &recent),
            Some("claude".to_string())
        );
    }

    #[test]
    fn select_preferred_provider_falls_back_to_first_available_provider() {
        let available = ids(&["goose", "claude"]);
        let recent = ids(&["codex"]);

        assert_eq!(
            select_preferred_provider(&available, &recent),
            Some("goose".to_string())
        );
    }

    #[test]
    fn resolve_preferred_provider_id_uses_explicit_provider() {
        assert_eq!(
            resolve_preferred_provider_id(
                Some("codex"),
                &ids(&["goose", "claude"]),
                &ids(&["claude"])
            ),
            Some("codex".to_string())
        );
    }

    #[test]
    fn resolve_preferred_provider_id_uses_recent_available_provider() {
        // Goose is first in KNOWN_AGENTS order, but the user's recent preference
        // is `claude` — the resolver must pick the preference, not first-installed.
        assert_eq!(
            resolve_preferred_provider_id(
                None,
                &ids(&["goose", "claude"]),
                &ids(&["codex", "claude"])
            ),
            Some("claude".to_string())
        );
    }

    #[test]
    fn resolve_preferred_provider_id_falls_back_to_first_available_provider() {
        // No recent agent is available, so fall back to the first available.
        assert_eq!(
            resolve_preferred_provider_id(None, &ids(&["goose", "claude"]), &ids(&["codex"])),
            Some("goose".to_string())
        );
    }

    #[test]
    fn resolve_preferred_provider_id_ignores_blank_explicit_provider() {
        assert_eq!(
            resolve_preferred_provider_id(
                Some("   "),
                &ids(&["goose", "claude"]),
                &ids(&["claude"])
            ),
            Some("claude".to_string())
        );
    }

    #[test]
    fn resolve_preferred_provider_id_returns_none_when_nothing_available() {
        assert_eq!(resolve_preferred_provider_id(None, &[], &[]), None);
    }

    #[test]
    fn resolve_provider_from_ids_rejects_unavailable_provider() {
        let available = ids(&["goose", "claude"]);

        let err = resolve_provider_from_ids(Some("codex".to_string()), &available, &[], true)
            .unwrap_err();

        assert!(err.contains("Selected agent provider `codex` is not available"));
    }

    #[test]
    fn resolve_provider_from_ids_requires_at_least_one_available_provider() {
        let err = resolve_provider_from_ids(None, &[], &[], false).unwrap_err();

        assert!(err.contains("No ACP agent found"));
    }

    #[test]
    fn infer_branch_resume_session_type_detects_pr_prompts() {
        assert_eq!(
            infer_branch_resume_session_type("Create a pull request for the current branch."),
            Some("pr")
        );
        assert_eq!(
            infer_branch_resume_session_type("Create a draft pull request for the current branch."),
            Some("pr")
        );
    }

    #[test]
    fn infer_branch_resume_session_type_detects_push_prompts() {
        assert_eq!(
            infer_branch_resume_session_type("Push the current branch to the remote."),
            Some("push")
        );
        assert_eq!(
            infer_branch_resume_session_type(
                "Push the current branch to the remote using force-with-lease."
            ),
            Some("push")
        );
    }

    #[test]
    fn infer_branch_resume_session_type_ignores_other_prompts() {
        assert_eq!(
            infer_branch_resume_session_type("Write a project note."),
            None
        );
    }

    fn assert_project_session_reference_guidance(prompt: &str) {
        assert!(prompt.contains("hashtag references in the form #<type>:<id>"));
        assert!(prompt.contains("#note:123"));
        assert!(prompt.contains("#commit:<sha>"));
        assert!(prompt.contains("#review:456"));
        assert!(prompt.contains("do not paste or rewrite the note contents"));
        assert!(prompt.contains("reference the note and relevant section instead"));
        assert!(prompt.contains("Implement \"Step 5: unit tests\" from #note:123"));
    }

    fn assert_project_session_repo_session_progress_guidance(prompt: &str) {
        assert!(prompt.contains("activity details"));
        assert!(prompt.contains("Prefer another `wait_for_repo_session` call"));
        assert!(prompt.contains("when the returned activity shows recent progress"));
        assert!(prompt.contains("when the user wants the session stopped"));
        assert!(prompt.contains("Cancellation is best used"));
        assert!(prompt.contains("go down a different path"));
        assert!(prompt.contains("surprised at how long the session is taking"));
        assert!(!prompt.contains("`last_activity_at`"));
        assert!(!prompt.contains("`last_tool_call`"));
        assert!(!prompt.contains("strong evidence"));
        assert!(!prompt.contains("repo session is taking a long time"));
    }

    fn assert_pikchr_note_guidance(prompt: &str, reference: &str) {
        assert!(prompt.contains("Staged notes support rendered diagrams"));
        assert!(prompt.contains("fenced `pikchr` code blocks"));
        assert!(prompt.contains("Pikchr grammar"));
        assert!(prompt.contains(reference));
    }

    #[test]
    fn generated_remote_pikchr_grammar_paths_are_unique_temp_markdown_files() {
        let first = generated_pikchr_grammar_remote_path();
        let second = generated_pikchr_grammar_remote_path();

        assert_ne!(first, second);
        for path in [first, second] {
            assert!(path.starts_with(PIKCHR_GRAMMAR_REMOTE_PATH_PREFIX));
            assert!(path.ends_with(PIKCHR_GRAMMAR_REMOTE_PATH_SUFFIX));

            let uuid_part = path
                .strip_prefix(PIKCHR_GRAMMAR_REMOTE_PATH_PREFIX)
                .and_then(|path| path.strip_suffix(PIKCHR_GRAMMAR_REMOTE_PATH_SUFFIX))
                .expect("generated path should contain a UUID between prefix and suffix");
            uuid::Uuid::parse_str(uuid_part).expect("generated path should include a UUID");
        }
    }

    #[test]
    fn successful_remote_pikchr_grammar_upload_returns_generated_path() {
        let expected_path = generated_pikchr_grammar_remote_path();
        let returned_path = upload_pikchr_grammar_to_remote_with_writer(
            "test-workspace",
            b"grammar bytes",
            expected_path.clone(),
            |workspace_name, bytes, remote_path| {
                assert_eq!(workspace_name, "test-workspace");
                assert_eq!(bytes, b"grammar bytes");
                assert_eq!(remote_path, expected_path);
                Ok(())
            },
        );

        assert_eq!(returned_path, expected_path);
    }

    #[test]
    fn failed_remote_pikchr_grammar_upload_falls_back_to_public_url() {
        let generated_path = generated_pikchr_grammar_remote_path();
        let returned_path = upload_pikchr_grammar_to_remote_with_writer(
            "test-workspace",
            b"grammar bytes",
            generated_path,
            |_workspace_name, _bytes, _remote_path| Err("remote unavailable".to_string()),
        );

        assert_eq!(returned_path, PIKCHR_GRAMMAR_URL);
    }

    #[test]
    fn local_project_session_prompt_includes_timeline_reference_guidance() {
        let prompt = build_project_session_action_instructions_with_pikchr_reference(
            false,
            PIKCHR_GRAMMAR_URL,
        );

        assert_project_session_reference_guidance(&prompt);
        assert_project_session_repo_session_progress_guidance(&prompt);
        assert_pikchr_note_guidance(&prompt, PIKCHR_GRAMMAR_URL);
    }

    #[test]
    fn remote_project_session_prompt_includes_timeline_reference_guidance() {
        let prompt = build_project_session_action_instructions_with_pikchr_reference(
            true,
            PIKCHR_GRAMMAR_URL,
        );

        assert_project_session_reference_guidance(&prompt);
        assert_project_session_repo_session_progress_guidance(&prompt);
        assert_pikchr_note_guidance(&prompt, PIKCHR_GRAMMAR_URL);
    }

    #[test]
    fn project_session_prompt_uses_supplied_pikchr_reference() {
        let prompt = build_project_session_action_instructions_with_pikchr_reference(
            false,
            "/tmp/staged/pikchr/grammar.md",
        );

        assert_pikchr_note_guidance(&prompt, "/tmp/staged/pikchr/grammar.md");
    }

    #[test]
    fn note_prompt_uses_supplied_pikchr_reference() {
        let prompt = build_full_prompt_with_pikchr_reference(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Note,
            None,
            None,
            "/tmp/staged/pikchr/grammar.md",
        );

        assert_pikchr_note_guidance(&prompt, "/tmp/staged/pikchr/grammar.md");
    }

    #[test]
    fn note_followup_prompt_uses_supplied_local_pikchr_reference() {
        let prompt = build_note_followup_message_with_pikchr_reference(
            true,
            "/Applications/Staged.app/Contents/Resources/resources/pikchr/grammar.md",
        );

        assert!(prompt.contains("The user is asking you to update the linked note"));
        assert!(prompt.contains("Please update the note to reflect the latest chat."));
        assert_pikchr_note_guidance(
            &prompt,
            "/Applications/Staged.app/Contents/Resources/resources/pikchr/grammar.md",
        );
    }

    #[test]
    fn note_followup_prompt_uses_supplied_remote_pikchr_reference() {
        let remote_path = generated_pikchr_grammar_remote_path();
        let prompt = build_note_followup_message_with_pikchr_reference(false, &remote_path);

        assert!(prompt.contains("The user is asking you to write the linked note"));
        assert!(prompt.contains("Please write the note for this session."));
        assert_pikchr_note_guidance(&prompt, &remote_path);
    }

    #[test]
    fn review_prompt_requires_strict_fence_lines() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Review,
            None,
            None,
        );

        assert!(
            prompt.contains("Then return your review comments as exactly one fenced JSON block:")
        );
        assert!(prompt
            .contains("The opening fence line for the title must be exactly: ```review-title"));
        assert!(prompt
            .contains("The opening fence line for comments must be exactly: ```review-comments"));
        assert!(prompt.contains("Each closing fence line must be exactly: ```"));
        assert!(prompt.contains(
            "Put only the JSON array inside the review-comments block (no prose or markdown)."
        ));
        assert!(prompt.contains("do not output any preamble, commentary, or thinking before it"));
    }

    #[test]
    fn review_prompt_includes_base_branch_in_diff_command() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Review,
            None,
            Some("develop"),
        );
        assert!(prompt.contains("git merge-base 'origin/develop' HEAD"));
        assert!(!prompt.contains("origin/main"));
    }

    #[test]
    fn review_prompt_defaults_to_main_when_no_base_branch() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Review,
            None,
            None,
        );
        assert!(prompt.contains("git merge-base 'origin/main' HEAD"));
    }

    #[test]
    fn review_prompt_normalizes_origin_prefixed_base_branch() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Review,
            None,
            Some("origin/main"),
        );
        // origin/main should NOT become origin/origin/main
        assert!(prompt.contains("git merge-base 'origin/main' HEAD"));
        assert!(!prompt.contains("origin/origin/main"));
    }

    #[test]
    fn review_prompt_shell_quotes_base_branch_with_single_quote() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Review,
            None,
            Some("feature/it's-good"),
        );
        assert!(prompt.contains("git merge-base 'origin/feature/it'\\''s-good' HEAD"));
        assert!(!prompt.contains("git merge-base 'origin/feature/it's-good' HEAD"));
    }

    #[test]
    fn review_timeline_entries_exclude_information_comments() {
        let (store, branch) = setup_branch_store();
        let review = create_branch_review(&store, &branch.id, "abc1234");
        add_agent_comment(
            &store,
            &review.id,
            "issue should appear",
            store::CommentType::Issue,
        );
        add_agent_comment(
            &store,
            &review.id,
            "warning should appear",
            store::CommentType::Warning,
        );
        add_agent_comment(
            &store,
            &review.id,
            "suggestion should appear",
            store::CommentType::Suggestion,
        );
        add_agent_comment(
            &store,
            &review.id,
            "information should be hidden",
            store::CommentType::Information,
        );

        let visible = HashSet::from(["abc1234".to_string()]);
        let entries = review_timeline_entries(&store, &branch.id, None, None, &visible);

        assert_eq!(entries.len(), 1);
        let content = &entries[0].content;
        assert!(content.contains("issue should appear"));
        assert!(content.contains("warning should appear"));
        assert!(content.contains("suggestion should appear"));
        assert!(!content.contains("information should be hidden"));
    }

    #[test]
    fn review_timeline_entries_skip_information_only_reviews() {
        let (store, branch) = setup_branch_store();
        let review = create_branch_review(&store, &branch.id, "abc1234");
        add_agent_comment(
            &store,
            &review.id,
            "information should be hidden",
            store::CommentType::Information,
        );

        let visible = HashSet::from(["abc1234".to_string()]);
        let entries = review_timeline_entries(&store, &branch.id, None, None, &visible);

        assert!(entries.is_empty());
    }

    #[test]
    fn review_timeline_entries_exclude_deleted_comments() {
        let (store, branch) = setup_branch_store();
        let review = create_branch_review(&store, &branch.id, "abc1234");
        let deleted = add_agent_comment(
            &store,
            &review.id,
            "deleted comment should be hidden",
            store::CommentType::Warning,
        );
        add_agent_comment(
            &store,
            &review.id,
            "active comment should appear",
            store::CommentType::Suggestion,
        );

        store.delete_comment(&deleted.id).unwrap();
        let visible = HashSet::from(["abc1234".to_string()]);
        let entries = review_timeline_entries(&store, &branch.id, None, None, &visible);

        assert_eq!(entries.len(), 1);
        let content = &entries[0].content;
        assert!(content.contains("active comment should appear"));
        assert!(!content.contains("deleted comment should be hidden"));
    }

    #[test]
    fn review_timeline_entries_hide_review_whose_commit_left_branch() {
        let (store, branch) = setup_branch_store();
        let review = create_branch_review(&store, &branch.id, "gone123");
        add_agent_comment(
            &store,
            &review.id,
            "issue on a rebased-away commit",
            store::CommentType::Issue,
        );

        // The review's commit is not among the branch's current SHAs, mirroring
        // a rebase/squash that dropped the original commit.
        let visible = HashSet::from(["still0n".to_string()]);
        let entries = review_timeline_entries(&store, &branch.id, None, None, &visible);

        assert!(entries.is_empty());
    }

    #[test]
    fn review_timeline_entries_keep_review_whose_commit_is_present() {
        let (store, branch) = setup_branch_store();
        let review = create_branch_review(&store, &branch.id, "present0");
        add_agent_comment(
            &store,
            &review.id,
            "issue on a current commit",
            store::CommentType::Issue,
        );

        let visible = HashSet::from(["present0".to_string()]);
        let entries = review_timeline_entries(&store, &branch.id, None, None, &visible);

        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.contains("issue on a current commit"));
    }

    #[test]
    fn review_timeline_entries_keep_review_with_user_comment_when_commit_gone() {
        let (store, branch) = setup_branch_store();
        let review = create_branch_review(&store, &branch.id, "gone123");
        let comment = store::Comment::new(
            "src/lib.rs",
            crate::git::Span::new(10, 10),
            "user kept this review alive",
        )
        .with_author(store::CommentAuthor::User)
        .with_comment_type(store::CommentType::Issue);
        store.add_comment(&review.id, &comment).unwrap();

        // Commit is gone, but a user comment keeps the review visible — matching
        // the branch card's `review_is_visible_in_timeline` rule.
        let visible = HashSet::from(["still0n".to_string()]);
        let entries = review_timeline_entries(&store, &branch.id, None, None, &visible);

        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.contains("user kept this review alive"));
    }

    #[test]
    fn parse_commit_shas_extracts_full_shas() {
        let log = "\u{0}1700000000\u{1}commit abc123def456\nAuthor: A\nDate: d\n\nfirst\
            \u{0}1700000100\u{1}commit 0011223344ff\nAuthor: B\nDate: d\n\nsecond";
        let shas = parse_commit_shas(log);

        assert_eq!(shas.len(), 2);
        assert!(shas.contains("abc123def456"));
        assert!(shas.contains("0011223344ff"));
    }

    #[test]
    fn old_review_summary_counts_exclude_information_comments() {
        let (store, branch) = setup_branch_store();
        let review = create_branch_review(&store, &branch.id, "abc1234");
        let temp_path = std::env::temp_dir().join(format!("staged-review-{}.md", review.id));
        let _ = std::fs::remove_file(&temp_path);

        add_agent_comment(
            &store,
            &review.id,
            "issue should appear",
            store::CommentType::Issue,
        );
        add_agent_comment(
            &store,
            &review.id,
            "warning should appear",
            store::CommentType::Warning,
        );
        add_agent_comment(
            &store,
            &review.id,
            "suggestion should appear",
            store::CommentType::Suggestion,
        );
        add_agent_comment(
            &store,
            &review.id,
            "information should be hidden",
            store::CommentType::Information,
        );

        let max_commit_ts = Some(review.created_at / 1000 + 1);
        let visible = HashSet::from(["abc1234".to_string()]);
        let entries = review_timeline_entries(&store, &branch.id, None, max_commit_ts, &visible);

        assert_eq!(entries.len(), 1);
        assert!(entries[0].content.contains("3 comments, 1 issues"));
        assert!(!entries[0].content.contains("4 comments"));
        assert!(entries[0].content.contains("See:"));

        let temp_content = std::fs::read_to_string(&temp_path).unwrap();
        assert!(temp_content.contains("issue should appear"));
        assert!(temp_content.contains("warning should appear"));
        assert!(temp_content.contains("suggestion should appear"));
        assert!(!temp_content.contains("information should be hidden"));

        let _ = std::fs::remove_file(&temp_path);
    }

    #[test]
    fn cancel_in_flight_auto_review_cancels_running_review() {
        let (store, branch) = setup_branch_store();
        let (session, review) =
            create_auto_review(&store, &branch.id, store::SessionStatus::Running);
        let registry = session_runner::SessionRegistry::new();

        let cancelled =
            cancel_in_flight_auto_review_for_branch(&store, &registry, &branch.id).unwrap();

        assert!(cancelled);
        // Session transitions to Cancelled but both records survive for potential adoption
        let session = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(session.status, store::SessionStatus::Cancelled);
        assert!(store.get_review(&review.id).unwrap().is_some());
    }

    #[test]
    fn cancel_in_flight_auto_review_cancels_queued_review() {
        let (store, branch) = setup_branch_store();
        let (session, review) =
            create_auto_review(&store, &branch.id, store::SessionStatus::Queued);
        let registry = session_runner::SessionRegistry::new();

        let cancelled =
            cancel_in_flight_auto_review_for_branch(&store, &registry, &branch.id).unwrap();

        assert!(cancelled);
        // Session transitions to Cancelled but both records survive for potential adoption
        let session = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(session.status, store::SessionStatus::Cancelled);
        assert!(store.get_review(&review.id).unwrap().is_some());
    }

    #[test]
    fn cancel_in_flight_auto_review_leaves_completed_review_available_for_adoption() {
        let (store, branch) = setup_branch_store();
        let (session, review) =
            create_auto_review(&store, &branch.id, store::SessionStatus::Completed);
        let registry = session_runner::SessionRegistry::new();

        let cancelled =
            cancel_in_flight_auto_review_for_branch(&store, &registry, &branch.id).unwrap();

        assert!(!cancelled);
        let session = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(session.status, store::SessionStatus::Completed);
        let review = store.get_review(&review.id).unwrap().unwrap();
        assert!(review.is_auto);
    }

    #[test]
    fn commit_prompt_appends_diff_viewer_context_to_branch_history() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Commit,
            Some(&BranchSessionLaunchContext {
                source: "diff_viewer".to_string(),
                scope: "commit".to_string(),
                commit_sha: "abc123".to_string(),
                review_id: Some("review-42".to_string()),
            }),
            None,
        );

        assert!(prompt.contains(
            "Viewed diff before starting this session: review review-42 on commit abc123 (scope: commit)."
        ));
    }

    #[test]
    fn commit_prompt_requires_global_identity_and_signoff() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Commit,
            None,
            None,
        );

        assert!(prompt.contains("Use the user's global git identity"));
        assert!(prompt.contains("git commit --signoff"));
        assert!(prompt.contains("Signed-off-by"));
    }

    #[test]
    fn commit_prompt_omits_branch_scope_from_diff_viewer_context() {
        let prompt = build_full_prompt(
            "user prompt",
            "project info",
            "branch context",
            &BranchSessionType::Commit,
            Some(&BranchSessionLaunchContext {
                source: "diff_viewer".to_string(),
                scope: "branch".to_string(),
                commit_sha: "abc123".to_string(),
                review_id: None,
            }),
            None,
        );

        assert!(prompt.contains("Viewed diff before starting this session: commit abc123."));
        assert!(!prompt.contains("(scope: branch)"));
    }

    #[test]
    fn queued_prompt_round_trips_launch_context() {
        let prompt = embed_launch_context(
            "Implement plan",
            Some(&BranchSessionLaunchContext {
                source: "diff_viewer".to_string(),
                scope: "branch".to_string(),
                commit_sha: "deadbeef".to_string(),
                review_id: None,
            }),
        )
        .unwrap();

        let (decoded_prompt, launch_context) = extract_launch_context(&prompt).unwrap();

        assert_eq!(decoded_prompt, "Implement plan");
        assert_eq!(
            launch_context,
            Some(BranchSessionLaunchContext {
                source: "diff_viewer".to_string(),
                scope: "branch".to_string(),
                commit_sha: "deadbeef".to_string(),
                review_id: None,
            })
        );
    }
}
