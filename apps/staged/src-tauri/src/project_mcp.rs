//! MCP server for project sessions.
//! Exposes `start_repo_session` and `add_project_repo` tools to the agent.

use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use axum::Router;
use base64::Engine;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use tauri::AppHandle;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::session_runner::SessionRegistry;
use crate::store::{
    AcpConfigSelection, Branch, CompletionReason, MessageRole, ProjectRepo, Session,
    SessionMessage, SessionStatus, Store,
};
use tokio_util::sync::CancellationToken;

/// What outcome the caller expects from a `start_repo_session` call.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RepoSessionOutcome {
    /// The session should produce a note in the repository. A note stub is created and
    /// the agent is instructed to output note content after a horizontal rule (---).
    NoteInRepo,
    /// The session should make code changes and create a commit. A pending commit record
    /// is created and the agent is instructed to commit with a signed-off conventional
    /// commit message.
    Commit,
    /// The session should run an AI code review of the changes on the repo's branch.
    /// A review record is created and populated with a confidence title and inline
    /// comments when the session completes.
    CodeReview,
    /// The session should produce a note attached to this project session's parent
    /// project note. Behaves like `NoteInRepo` (a note stub bound to the spawned repo
    /// session) but the note is additionally linked to the parent via
    /// `parent_project_note_id`, so it is hidden from the repo's visible timeline and
    /// aggregated under the parent (referenced as `#note:<id>`).
    ChildNote,
}

/// Build the note stub for a `note_in_repo` / `child_note` repo-session outcome.
///
/// The note is bound to the spawned repo session. When `parent_project_note_id`
/// is `Some` (a `child_note` outcome with a parent project note in scope), the
/// note is additionally attached to that parent so it is aggregated under it and
/// excluded from the repo's visible timeline. When `None`, the note falls back to
/// a plain detached note — identical to a `note_in_repo` outcome — rather than
/// erroring; this is the safe behavior when a `child_note` is requested without a
/// parent in scope (e.g. a non-project session).
fn build_repo_note_stub(
    branch_id: &str,
    instructions: &str,
    session_id: &str,
    parent_project_note_id: Option<&str>,
) -> crate::store::Note {
    let mut note = crate::store::Note::new(branch_id, instructions, "").with_session(session_id);
    if let Some(parent_id) = parent_project_note_id {
        note = note.with_parent_project_note(parent_id);
    }
    note
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct StartRepoSessionParams {
    /// GitHub repo slug present in the project, e.g. "org/repo".
    pub repo: String,
    /// Subpath within the repository (for monorepos), e.g. "packages/api".
    /// Must match exactly the subpath used when the repo was added to the project.
    /// Use `null` / omit if the repo was added without a subpath (whole-repo).
    pub subpath: Option<String>,
    /// Instructions to give the agent. Notes previously created for this repo are available
    /// to the session, so you can refer to them by name (e.g. "refer to the architecture
    /// overview note").
    ///
    /// For `"code_review"` leave this empty for a standard review of the branch's
    /// changes. Provide instructions only when there is something specific you want
    /// looked into or have concerns about (e.g. "focus on the migration ordering").
    pub instructions: String,
    /// What the session should produce. Controls the prompt given to the agent and what
    /// artifact (if any) is created in the database.
    ///
    /// - `"note_in_repo"`: Use this for generating notes that can be referred to again
    ///   later by other sessions or by the user. Useful for architecture overviews, plans,
    ///   research.
    /// - `"commit"`: Use this to request code changes. Agent makes code changes and
    ///   creates a signed-off commit with a conventional commit message.
    /// - `"code_review"`: Use this to request an AI code review of the changes on the
    ///   repo's branch. Produces a review with a confidence title and inline comments
    ///   anchored to the diff.
    pub expected_outcome: RepoSessionOutcome,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct WaitForRepoSessionParams {
    /// Opaque handle returned by `start_repo_session`.
    pub repo_session_id: String,
    /// How long to wait for a status change before returning the current state.
    #[serde(default = "default_wait_for_completion_seconds")]
    pub wait_for_completion_seconds: u64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct CancelRepoSessionParams {
    /// Opaque handle returned by `start_repo_session`.
    pub repo_session_id: String,
}

fn default_wait_for_completion_seconds() -> u64 {
    240
}

const REPO_SESSION_ACTIVITY_PREVIEW_MAX_CHARS: usize = 240;

#[derive(Debug, Default, serde::Serialize)]
struct RepoSessionActivity {
    last_activity_at: Option<i64>,
    last_message: Option<RepoSessionActivityEntry>,
    last_tool_call: Option<RepoSessionActivityEntry>,
    last_tool_result: Option<RepoSessionActivityEntry>,
    counts: RepoSessionActivityCounts,
}

#[derive(Debug, serde::Serialize)]
struct RepoSessionActivityEntry {
    role: String,
    created_at: i64,
    preview: String,
}

#[derive(Debug, Default, serde::Serialize)]
struct RepoSessionActivityCounts {
    assistant_messages: usize,
    tool_calls: usize,
    tool_results: usize,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum RepoArtifactKind {
    Note,
    Commit,
    Review,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RepoSessionHandle {
    session_id: String,
    artifact_kind: RepoArtifactKind,
    artifact_id: String,
}

struct ResolvedRepoTarget {
    repo: ProjectRepo,
    branch: Branch,
}

/// The `start_line` / `end_line` pair reported for a review comment.
///
/// Comment spans are stored the way the review prompt asks for them: 0-indexed
/// lines from the "after" side of the diff with an exclusive end. The payload
/// reports 1-indexed inclusive lines instead — what the field names imply to
/// the reading agent, and the same conversion review comments already get when
/// they're posted to GitHub. An empty span (`start == end`, an anchor covering
/// no lines) collapses to the single line it sits on rather than reporting an
/// end before the start.
fn comment_line_range(span: crate::git::Span) -> (u32, u32) {
    let start_line = span.start.saturating_add(1);
    (start_line, span.end.max(start_line))
}

/// The ACP config selection a queued repo session should carry.
///
/// The parent project session's selection names config and value IDs that only
/// exist on the parent's own provider, so it is inherited only when the repo
/// session runs on that provider. When review provider resolution falls back
/// to a different agent, the selection is dropped: the fallback agent would
/// fail config application at session start, and because a retried
/// `start_repo_session` stamps the handler's selection onto a fresh row, the
/// failure would repeat on every retry rather than self-heal.
fn inherited_acp_config_selection(
    inherited_provider: Option<&str>,
    session_provider: Option<&str>,
    selection: Option<&AcpConfigSelection>,
) -> Option<AcpConfigSelection> {
    if session_provider == inherited_provider {
        selection.cloned()
    } else {
        None
    }
}

impl ProjectToolsHandler {
    fn resolve_repo_target(
        &self,
        repo_slug: &str,
        subpath: Option<&str>,
    ) -> Result<ResolvedRepoTarget, String> {
        let repos = self
            .store
            .list_project_repos(&self.project_id)
            .map_err(|e| format!("Error listing repos: {e}"))?;
        let repo = repos
            .iter()
            .find(|r| r.github_repo == repo_slug && r.subpath.as_deref() == subpath)
            .cloned()
            .ok_or_else(|| {
                let available = repos
                    .iter()
                    .map(|r| match r.subpath.as_deref() {
                        Some(sp) => format!("{} (subpath: {sp})", r.github_repo),
                        None => r.github_repo.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "Repository '{}' with subpath {:?} not found in project. Available repos: {}",
                    repo_slug, subpath, available
                )
            })?;

        let branches = self
            .store
            .list_branches_for_project(&self.project_id)
            .map_err(|e| format!("Error listing branches: {e}"))?;
        let branch = branches
            .into_iter()
            .find(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str()))
            .ok_or_else(|| {
                format!(
                    "No branch found for repo '{}'. Ensure the repo has been fully set up via add_project_repo before starting a session.",
                    repo.github_repo
                )
            })?;

        if branch.workspace_name.is_none() {
            let has_workdir = self
                .store
                .get_workdir_for_branch(&branch.id)
                .map_err(|e| format!("Error looking up worktree: {e}"))?
                .is_some();
            if !has_workdir {
                return Err(format!(
                    "No worktree found for repo '{}'. Ensure the repo has been fully set up via add_project_repo before starting a session.",
                    repo.github_repo
                ));
            }
        }

        Ok(ResolvedRepoTarget { repo, branch })
    }

    fn encode_repo_session_handle(
        session_id: &str,
        artifact_kind: RepoArtifactKind,
        artifact_id: &str,
    ) -> Result<String, String> {
        let payload = serde_json::to_vec(&RepoSessionHandle {
            session_id: session_id.to_string(),
            artifact_kind,
            artifact_id: artifact_id.to_string(),
        })
        .map_err(|e| format!("Failed to encode repo session handle: {e}"))?;
        Ok(format!(
            "repo_session_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
        ))
    }

    fn decode_repo_session_handle(repo_session_id: &str) -> Result<RepoSessionHandle, String> {
        let encoded = repo_session_id
            .strip_prefix("repo_session_")
            .ok_or_else(|| format!("Invalid repo_session_id: {repo_session_id}"))?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|e| format!("Invalid repo_session_id: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid repo_session_id: {e}"))
    }

    fn session_state(status: SessionStatus) -> &'static str {
        match status {
            SessionStatus::Queued => "queued",
            SessionStatus::Running => "running",
            SessionStatus::Completed => "completed",
            SessionStatus::Cancelled => "cancelled",
            SessionStatus::Error => "failed",
        }
    }

    fn last_assistant_output(&self, session_id: &str) -> String {
        self.store
            .get_session_messages(session_id)
            .ok()
            .and_then(|msgs| {
                msgs.into_iter()
                    .rfind(|m| m.role == crate::store::MessageRole::Assistant)
                    .map(|m| m.content)
            })
            .unwrap_or_default()
    }

    fn activity_preview(content: &str) -> String {
        let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut chars = normalized.chars();
        let mut preview = String::new();

        for _ in 0..REPO_SESSION_ACTIVITY_PREVIEW_MAX_CHARS {
            match chars.next() {
                Some(ch) => preview.push(ch),
                None => return preview,
            }
        }

        if chars.next().is_some() {
            preview.push_str("...");
        }

        preview
    }

    fn activity_entry(message: &SessionMessage) -> RepoSessionActivityEntry {
        RepoSessionActivityEntry {
            role: message.role.as_str().to_string(),
            created_at: message.created_at,
            preview: Self::activity_preview(&message.content),
        }
    }

    fn summarize_session_activity(
        session: &Session,
        messages: &[SessionMessage],
    ) -> RepoSessionActivity {
        let mut activity = RepoSessionActivity {
            last_activity_at: Some(session.created_at.max(session.updated_at)),
            ..Default::default()
        };

        for message in messages {
            activity.last_activity_at = Some(
                activity
                    .last_activity_at
                    .map_or(message.created_at, |at| at.max(message.created_at)),
            );

            let entry = Self::activity_entry(message);
            activity.last_message = Some(RepoSessionActivityEntry {
                role: entry.role.clone(),
                created_at: entry.created_at,
                preview: entry.preview.clone(),
            });

            match &message.role {
                MessageRole::Assistant => {
                    activity.counts.assistant_messages += 1;
                }
                MessageRole::ToolCall => {
                    activity.counts.tool_calls += 1;
                    activity.last_tool_call = Some(entry);
                }
                MessageRole::ToolResult => {
                    activity.counts.tool_results += 1;
                    activity.last_tool_result = Some(entry);
                }
                MessageRole::User => {}
            }
        }

        activity
    }

    fn repo_session_activity(&self, session: &Session) -> Result<RepoSessionActivity, String> {
        let messages = self
            .store
            .get_session_messages(&session.id)
            .map_err(|e| format!("Error loading session messages: {e}"))?;
        Ok(Self::summarize_session_activity(session, &messages))
    }

    fn repo_session_payload(
        &self,
        repo_session_id: &str,
        handle: &RepoSessionHandle,
    ) -> Result<serde_json::Value, String> {
        let session = self
            .store
            .get_session(&handle.session_id)
            .map_err(|e| format!("Error loading repo session: {e}"))?
            .ok_or_else(|| format!("Repo session not found for id: {repo_session_id}"))?;
        let session_status = session.status.clone();
        let state = Self::session_state(session_status.clone());
        let mut payload = serde_json::json!({
            "repo_session_id": repo_session_id,
            "state": state,
            "artifact": {
                "type": match handle.artifact_kind {
                    RepoArtifactKind::Note => "note",
                    RepoArtifactKind::Commit => "commit",
                    RepoArtifactKind::Review => "review",
                },
                "id": handle.artifact_id,
            },
        });

        payload["activity"] = serde_json::json!(self.repo_session_activity(&session)?);

        if state != "queued" {
            payload["session_id"] = serde_json::Value::String(handle.session_id.clone());
        }
        if let Some(reason) = session.completion_reason.as_ref() {
            payload["completion_reason"] = serde_json::Value::String(reason.as_str().to_string());
        }
        if let Some(error) = session.error_message.as_ref() {
            payload["error_message"] = serde_json::Value::String(error.clone());
        }

        if matches!(
            session_status,
            SessionStatus::Completed | SessionStatus::Cancelled | SessionStatus::Error
        ) {
            match handle.artifact_kind {
                RepoArtifactKind::Note => {
                    let note = self
                        .store
                        .get_note(&handle.artifact_id)
                        .map_err(|e| format!("Error loading note: {e}"))?;
                    if let Some(note) = note {
                        payload["note"] = serde_json::json!({
                            "id": note.id,
                            "title": note.title,
                            "content": note.content,
                            "completed_at": note.completed_at,
                        });
                    }
                }
                RepoArtifactKind::Commit => {
                    let commit = self
                        .store
                        .get_commit(&handle.artifact_id)
                        .map_err(|e| format!("Error loading commit: {e}"))?;
                    if let Some(commit) = commit {
                        payload["commit"] = serde_json::json!({
                            "id": commit.id,
                            "sha": commit.sha,
                        });
                    }
                }
                RepoArtifactKind::Review => {
                    let review = self
                        .store
                        .get_review(&handle.artifact_id)
                        .map_err(|e| format!("Error loading review: {e}"))?;
                    if let Some(review) = review {
                        payload["review"] = serde_json::json!({
                            "id": review.id,
                            "title": review.title,
                            "completed_at": review.completed_at,
                            "comments": review
                                .comments
                                .iter()
                                .map(|c| {
                                    let (start_line, end_line) = comment_line_range(c.span);
                                    serde_json::json!({
                                        "path": c.path,
                                        "start_line": start_line,
                                        "end_line": end_line,
                                        "type": c.comment_type.as_ref().map(|t| t.as_str()),
                                        "content": c.content,
                                    })
                                })
                                .collect::<Vec<_>>(),
                        });
                    }
                }
            }

            payload["output"] =
                serde_json::Value::String(self.last_assistant_output(&handle.session_id));
        }

        Ok(payload)
    }

    /// Returns true if the store still records this session as running. Used to
    /// decide whether to defer a cancellation during the startup window where
    /// the DB row is already `running` but the runner hasn't registered its
    /// cancellation token yet.
    fn session_is_running(&self, session_id: &str) -> bool {
        matches!(
            self.store.get_session(session_id),
            Ok(Some(session)) if session.status == SessionStatus::Running
        )
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddProjectRepoParams {
    /// GitHub repo slug to add, e.g. "org/repo".
    pub github_repo: String,
    /// Optional branch name (defaults to project's inferred name).
    pub branch_name: Option<String>,
    /// Subpath to the specific service or project within the repository.
    /// Required for monorepos — you must provide the path to the root of the
    /// relevant service or package (e.g. "packages/api" or "services/auth").
    /// If omitted for a regular repo, the whole repository is used.
    pub subpath: Option<String>,
    /// Reason this repository is being added to the project. Shown to the user
    /// in the branch card timeline so they understand why it was added. Describe what
    /// the repo is and how it relates to the project — do not include todos or details
    /// about what needs to change.
    pub reason: Option<String>,
    /// Base branch to branch off from (e.g. "main", "develop"). If omitted,
    /// the repository's default branch is detected automatically via the
    /// GitHub API.
    pub base_branch: Option<String>,
}

#[derive(Clone)]
struct ProjectToolsHandler {
    tool_router: ToolRouter<Self>,
    project_id: String,
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
    action_executor: Option<Arc<ActionExecutor>>,
    action_registry: Option<Arc<ActionRegistry>>,
    /// ACP provider ID inherited from the parent project session.
    /// All repo sessions spawned by this handler use this provider.
    provider: Option<String>,
    /// ACP config selection inherited from the parent project session.
    /// Repo sessions persist it when queued so queue drain uses the selection
    /// active when the parent requested the work.
    acp_config_selection: Option<AcpConfigSelection>,
    /// Project note id of the parent project session, when this handler serves a
    /// project session. Repo sessions started with `expected_outcome="child_note"`
    /// attach their note to this parent so it is aggregated under it and hidden from
    /// the repo timeline. `None` for handlers without a parent project note in scope.
    parent_project_note_id: Option<String>,
    /// Cancellation token for the parent project session.
    /// Signalled when the user cancels the project session.
    cancel_token: CancellationToken,
}

impl ProjectToolsHandler {
    #[allow(clippy::too_many_arguments)]
    fn new(
        project_id: String,
        store: Arc<Store>,
        registry: Arc<SessionRegistry>,
        app_handle: AppHandle,
        action_executor: Option<Arc<ActionExecutor>>,
        action_registry: Option<Arc<ActionRegistry>>,
        provider: Option<String>,
        acp_config_selection: Option<AcpConfigSelection>,
        parent_project_note_id: Option<String>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            project_id,
            store,
            registry,
            app_handle,
            action_executor,
            action_registry,
            provider,
            acp_config_selection,
            parent_project_note_id,
            cancel_token,
        }
    }
}

#[tool_router]
impl ProjectToolsHandler {
    #[tool(
        description = "Enqueue an agent session in one of the project's repositories and return immediately with an opaque `repo_session_id`. Use `expected_outcome=\"note_in_repo\"` for repo notes, `expected_outcome=\"commit\"` for code changes and a signed-off conventional commit, or `expected_outcome=\"code_review\"` for an AI code review of the changes on the repo's branch. The `repo` + `subpath` combination must exactly match an entry already in the project."
    )]
    async fn start_repo_session(
        &self,
        Parameters(p): Parameters<StartRepoSessionParams>,
    ) -> String {
        log::debug!(
            "[project_mcp] start_repo_session called: repo={:?} subpath={:?} expected_outcome={:?} provider={:?} instructions={:?}",
            p.repo,
            p.subpath,
            p.expected_outcome,
            self.provider,
            p.instructions,
        );
        let target = match self.resolve_repo_target(&p.repo, p.subpath.as_deref()) {
            Ok(target) => target,
            Err(e) => return e,
        };

        // Review sessions only run on review-capable providers, so resolve (and
        // validate) the provider before creating any rows — a failure at drain
        // time would strand a queued session the drain loop can never start.
        // The provider is inherited from the parent project session rather than
        // chosen for the review, so one that can't review (e.g. an agent that
        // isn't available on remote workstations) falls back to the preferred
        // review-capable provider instead of failing a call the agent has no
        // provider parameter to work around.
        let session_provider = if matches!(p.expected_outcome, RepoSessionOutcome::CodeReview) {
            match crate::session_commands::resolve_inherited_review_provider(
                self.provider.clone(),
                target.branch.workspace_name.is_some(),
            )
            .await
            {
                Ok(provider) => Some(provider),
                Err(e) => return e,
            }
        } else {
            self.provider.clone()
        };

        // An in-flight auto review of the same branch would duplicate a requested
        // review, and is invalidated by a commit (which triggers a fresh auto review
        // once it lands), so cancel it first — same as user-initiated sessions.
        if matches!(
            p.expected_outcome,
            RepoSessionOutcome::CodeReview | RepoSessionOutcome::Commit
        ) {
            if let Err(e) = crate::session_commands::cancel_in_flight_auto_review_for_branch(
                &self.store,
                &self.registry,
                &target.branch.id,
            ) {
                return format!("Error cancelling in-flight auto review: {e}");
            }
        }

        let mut session = crate::store::Session::new_queued(&p.instructions);
        if let Some(ref provider) = session_provider {
            session = session.with_provider(provider);
        }
        if let Some(selection) = inherited_acp_config_selection(
            self.provider.as_deref(),
            session_provider.as_deref(),
            self.acp_config_selection.as_ref(),
        ) {
            session = session.with_acp_config_selection(selection);
        }
        if let Err(e) = self.store.create_session(&session) {
            return format!("Error creating queued session: {e}");
        }

        let (artifact_id, artifact_kind) = match p.expected_outcome {
            RepoSessionOutcome::NoteInRepo => {
                let note =
                    build_repo_note_stub(&target.branch.id, &p.instructions, &session.id, None);
                let note_id = note.id.clone();
                if let Err(e) = self.store.create_note(&note) {
                    return format!("Error creating note stub: {e}");
                }
                (note_id, RepoArtifactKind::Note)
            }
            RepoSessionOutcome::ChildNote => {
                // Attach the note to the parent project note so it is aggregated
                // under it and hidden from the repo timeline. With no parent in
                // scope (e.g. a non-project session), fall back to a plain
                // detached note rather than erroring.
                let note = build_repo_note_stub(
                    &target.branch.id,
                    &p.instructions,
                    &session.id,
                    self.parent_project_note_id.as_deref(),
                );
                let note_id = note.id.clone();
                if let Err(e) = self.store.create_note(&note) {
                    return format!("Error creating child note stub: {e}");
                }
                (note_id, RepoArtifactKind::Note)
            }
            RepoSessionOutcome::Commit => {
                let commit =
                    crate::store::Commit::new_pending(&target.branch.id).with_session(&session.id);
                let commit_id = commit.id.clone();
                if let Err(e) = self.store.create_commit(&commit) {
                    return format!("Error creating commit stub: {e}");
                }
                (commit_id, RepoArtifactKind::Commit)
            }
            RepoSessionOutcome::CodeReview => {
                // The commit_sha is filled in at drain time, when the workspace
                // exists and the branch tip can be read (same as queued user
                // review sessions).
                let review = crate::store::Review::new(
                    &target.branch.id,
                    "",
                    crate::store::ReviewScope::Branch,
                )
                .with_session(&session.id);
                let review_id = review.id.clone();
                if let Err(e) = self.store.create_review(&review) {
                    return format!("Error creating review stub: {e}");
                }
                (review_id, RepoArtifactKind::Review)
            }
        };

        let repo_session_id =
            match Self::encode_repo_session_handle(&session.id, artifact_kind, &artifact_id) {
                Ok(id) => id,
                Err(e) => return e,
            };

        // The inherited provider, not the review-resolved one: this argument is the
        // branch-wide default the drain pass applies to every queued session without
        // a stored provider, so a review's fallback agent must not pull unrelated
        // queued sessions off the project session's provider. The session created
        // above carries its own resolved provider on its row.
        if let Err(e) = crate::session_commands::drain_queued_sessions_for_branch(
            Arc::clone(&self.store),
            Arc::clone(&self.registry),
            self.app_handle.clone(),
            target.branch.id.clone(),
            self.provider.clone(),
        )
        .await
        {
            return format!("Error draining queued sessions: {e}");
        }

        serde_json::json!({
            "repo_session_id": repo_session_id,
            "state": "queued",
            "artifact": {
                "type": match artifact_kind {
                    RepoArtifactKind::Note => "note",
                    RepoArtifactKind::Commit => "commit",
                    RepoArtifactKind::Review => "review",
                },
                "id": artifact_id,
            },
            "repo": target.repo.github_repo,
            "subpath": target.repo.subpath,
            "branch_id": target.branch.id,
            "message": format!(
                "Repo session enqueued. Wait with wait_for_repo_session({{\"repo_session_id\":\"{}\",\"wait_for_completion_seconds\":240}}).",
                repo_session_id
            ),
        })
        .to_string()
    }

    #[tool(
        description = "Wait for a repo session started by `start_repo_session`. Returns the current state and any available artifacts for the opaque `repo_session_id`. `wait_for_completion_seconds` defaults to 240."
    )]
    async fn wait_for_repo_session(
        &self,
        Parameters(p): Parameters<WaitForRepoSessionParams>,
        request_ct: CancellationToken,
    ) -> String {
        let handle = match Self::decode_repo_session_handle(&p.repo_session_id) {
            Ok(handle) => handle,
            Err(e) => return e,
        };

        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(p.wait_for_completion_seconds);
        loop {
            match self.repo_session_payload(&p.repo_session_id, &handle) {
                Ok(payload) => {
                    let state = payload
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("failed");
                    if matches!(state, "completed" | "cancelled" | "failed") {
                        return payload.to_string();
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return payload.to_string();
                    }
                }
                Err(e) => return e,
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = request_ct.cancelled() => {
                    return match self.repo_session_payload(&p.repo_session_id, &handle) {
                        Ok(payload) => payload.to_string(),
                        Err(e) => e,
                    };
                }
                _ = self.cancel_token.cancelled() => {
                    return match self.repo_session_payload(&p.repo_session_id, &handle) {
                        Ok(payload) => payload.to_string(),
                        Err(e) => e,
                    };
                }
            }
        }
    }

    #[tool(
        description = "Abort a repo session started by `start_repo_session` when the user wants the session stopped. Cancellation is best used when the user wants to go down a different path rather than when you are surprised at how long the session is taking. Cancels either the queued item or the running session behind the opaque `repo_session_id`."
    )]
    async fn cancel_repo_session(
        &self,
        Parameters(p): Parameters<CancelRepoSessionParams>,
    ) -> String {
        let handle = match Self::decode_repo_session_handle(&p.repo_session_id) {
            Ok(handle) => handle,
            Err(e) => return e,
        };

        // Atomically try to cancel from queued state first. If the session
        // was already picked up by the drain loop (transitioned to running),
        // this returns false and we fall through to the running-cancel path.
        let was_queued = match self.store.transition_from_queued(
            &handle.session_id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::ProjectSessionInterrupted),
        ) {
            Ok(updated) => updated,
            Err(e) => return format!("Error cancelling repo session: {e}"),
        };

        if was_queued {
            // Successfully cancelled from queued state. Drain the next
            // queued session for this branch so it can start.
            if let Ok(Some(branch_id)) = self.store.get_branch_id_for_session(&handle.session_id) {
                crate::web_server::emit_to_all(
                    &self.app_handle,
                    "session-status-changed",
                    crate::session_runner::SessionStatusEvent {
                        session_id: handle.session_id.clone(),
                        status: "cancelled".to_string(),
                        error_message: None,
                        completion_reason: Some(
                            CompletionReason::ProjectSessionInterrupted
                                .as_str()
                                .to_string(),
                        ),
                        branch_id: Some(branch_id.clone()),
                        project_id: Some(self.project_id.clone()),
                        session_type: None,
                        is_auto_review: false,
                    },
                );
                let _ = crate::session_commands::drain_queued_sessions_for_branch(
                    Arc::clone(&self.store),
                    Arc::clone(&self.registry),
                    self.app_handle.clone(),
                    branch_id,
                    None,
                )
                .await;
            }
        } else {
            // Session is running (or already terminal). Ask the runner to cancel.
            //
            // There is a startup race: a queued session becomes `running` in the
            // DB (so `transition_from_queued` returns false above) a moment before
            // `start_session` registers its cancellation token. In that window the
            // registry doesn't yet know about the session and the immediate cancel
            // attempt returns false, which would silently drop the cancellation.
            let cancelled = self.registry.cancel_with_completion_reason(
                &handle.session_id,
                CompletionReason::ProjectSessionInterrupted,
            );
            // If the immediate cancel found nothing but the DB still records the
            // session as running, we're in that startup window. Defer the
            // cancellation so `start_session` applies it the instant it registers
            // its token, guaranteeing the cancellation lands however long startup
            // takes instead of racing a fixed-delay retry that a slow remote
            // startup could outlast. The DB gate keeps an already-terminal
            // session from leaving a stale pending entry behind.
            if !cancelled && self.session_is_running(&handle.session_id) {
                self.registry.cancel_or_defer(
                    &handle.session_id,
                    CompletionReason::ProjectSessionInterrupted,
                );
            }
        }

        match self.repo_session_payload(&p.repo_session_id, &handle) {
            Ok(payload) => payload.to_string(),
            Err(e) => e,
        }
    }

    #[tool(
        description = "Add a GitHub repository to the current project. Use this when the task requires a repository that isn't yet in the project. Returns once the repository worktree is ready; any setup actions then run in the background."
    )]
    async fn add_project_repo(&self, Parameters(p): Parameters<AddProjectRepoParams>) -> String {
        log::debug!(
            "[project_mcp] add_project_repo called: github_repo={:?} branch_name={:?} subpath={:?} base_branch={:?}",
            p.github_repo,
            p.branch_name,
            p.subpath,
            p.base_branch,
        );

        // If no subpath was provided, check whether the repo is a monorepo.
        // Monorepos require a subpath to identify which service/package to use.
        if p.subpath.is_none() {
            let repo_slug = p.github_repo.clone();
            let monorepo_result = tauri::async_runtime::spawn_blocking(move || {
                crate::git::check_monorepo_modules(&repo_slug)
            })
            .await;

            match monorepo_result {
                Ok(Ok(module_count)) if module_count >= 20 => {
                    return format!(
                        "Error: '{}' appears to be a monorepo ({} modules in MODULES.yaml). \
                         You must provide a `subpath` pointing to the root of the specific \
                         service or package you want to add (e.g. \"packages/api\" or \
                         \"services/auth\"). Re-call this tool with the appropriate subpath.",
                        p.github_repo, module_count
                    );
                }
                Ok(Err(e)) => {
                    log::warn!(
                        "[project_mcp] monorepo check failed for {}: {e}",
                        p.github_repo
                    );
                }
                Err(e) => {
                    log::warn!("[project_mcp] monorepo check task panicked: {e}");
                }
                Ok(Ok(_)) => {} // fewer than 20 modules, not a monorepo
            }
        }

        // Detect fork repos: if the provided github_repo is a fork, use the
        // parent (upstream) repo for cloning/API calls and record the fork as
        // head_repo so the UI displays the correct source.
        let (effective_repo, head_repo) = {
            let slug = p.github_repo.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                crate::git::github::get_parent_repo(&slug)
            })
            .await
            {
                Ok(Ok(Some(parent))) => {
                    log::info!(
                        "[project_mcp] detected fork: {} -> parent {}",
                        p.github_repo,
                        parent
                    );
                    (parent, Some(p.github_repo.clone()))
                }
                Ok(Ok(None)) => (p.github_repo.clone(), None),
                Ok(Err(e)) => {
                    log::warn!(
                        "[project_mcp] failed to check if {} is a fork: {e}",
                        p.github_repo
                    );
                    (p.github_repo.clone(), None)
                }
                Err(e) => {
                    log::warn!("[project_mcp] fork check task panicked: {e}");
                    (p.github_repo.clone(), None)
                }
            }
        };

        let github_repo = effective_repo.clone();
        let repo = match crate::project_commands::add_project_repo_impl(
            Arc::clone(&self.store),
            self.project_id.clone(),
            effective_repo,
            p.branch_name,
            p.subpath,
            None,
            p.reason,
            None,
            p.base_branch,
            head_repo,
        )
        .await
        {
            Ok(repo) => repo,
            Err(e) => return format!("Error adding repo: {e}"),
        };

        // Notify the UI so the repo appears immediately
        crate::web_server::emit_to_all(
            &self.app_handle,
            "project-setup-progress",
            self.project_id.clone(),
        );

        // Find the branch that was just created for this repo
        let branch = match self.store.list_branches_for_project(&self.project_id) {
            Ok(branches) => branches
                .into_iter()
                .find(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str())),
            Err(e) => return format!("Error listing branches after adding repo: {e}"),
        };

        let branch = match branch {
            Some(b) => b,
            None => {
                // Repo was added but no branch found — return partial success
                log::warn!(
                    "[project_mcp] add_project_repo: no branch found for repo {} after creation",
                    github_repo
                );
                return format!("Added repository {github_repo} to project (no branch found)");
            }
        };

        // For local branches, set up the git worktree
        if branch.workspace_name.is_none() {
            log::debug!(
                "[project_mcp] add_project_repo: setting up worktree for branch {}",
                branch.branch_name
            );
            let branch_id = branch.id.clone();
            let store = Arc::clone(&self.store);
            let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                // We need to run the worktree setup synchronously
                // Reuse the core logic from branches::setup_worktree
                crate::branches::setup_worktree_sync(&store, &branch_id, None)
            })
            .await;

            let worktree_path = match worktree_result {
                Ok(Ok(path)) => {
                    log::debug!("[project_mcp] add_project_repo: worktree ready at {}", path);
                    // Notify UI that the worktree is ready so branch state updates
                    crate::web_server::emit_to_all(
                        &self.app_handle,
                        "project-setup-progress",
                        self.project_id.clone(),
                    );
                    path
                }
                Ok(Err(e)) => {
                    log::warn!(
                        "[project_mcp] add_project_repo: worktree setup failed (continuing): {e}"
                    );
                    // Don't abort — return the repo even if worktree setup failed
                    return format!(
                        "Added repository {github_repo} to project (worktree setup failed: {e})"
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[project_mcp] add_project_repo: worktree task panicked (continuing): {e}"
                    );
                    return format!(
                        "Added repository {github_repo} to project (worktree task error: {e})"
                    );
                }
            };

            // Prerun waits out any in-flight action detection and then runs
            // each setup action to completion — minutes, against an MCP
            // client's request timeout. Nothing in the reply derives from it,
            // so the whole tail is detached, in one task so the auto-review
            // still follows the setup actions. Structurally this is now the
            // Tauri `add_project_repo` command's spawned setup task. Note that
            // the reply therefore also lands before the auto-review is queued.
            let store = Arc::clone(&self.store);
            let app_handle = self.app_handle.clone();
            let project_id = self.project_id.clone();
            let branch_id = branch.id.clone();
            let executor = self.action_executor.clone();
            let act_registry = self.action_registry.clone();
            tauri::async_runtime::spawn(async move {
                // Run detect_actions + prerun actions if we have an executor
                if let (Some(executor), Some(act_registry)) = (executor, act_registry) {
                    if let crate::branches::PrerunOutcome::Ran(_) =
                        crate::branches::claim_and_run_prerun_actions(
                            &store,
                            &app_handle,
                            &branch_id,
                            &executor,
                            &act_registry,
                            None,
                            "project_mcp add_project_repo",
                        )
                        .await
                    {
                        // Notify UI that prerun actions finished
                        crate::web_server::emit_to_all(
                            &app_handle,
                            "project-setup-progress",
                            project_id,
                        );
                    }
                } else {
                    log::debug!(
                        "[project_mcp] add_project_repo: no action executor available, skipping prerun actions"
                    );
                }

                // If the repo already has commits on this branch, kick off
                // an automatic code review so the user gets immediate feedback.
                crate::maybe_trigger_auto_review_for_new_repo(
                    &store,
                    &app_handle,
                    &branch_id,
                    Some(&worktree_path),
                )
                .await;
            });

            // The reply is the agent's whole account of what just happened, so
            // it only promises setup actions when the task above can actually
            // run them. Whether the executor is there is known right here,
            // synchronously — the task's other way out (losing the one-shot
            // setup claim) isn't, but the branch was created moments ago by
            // this call, so nothing else has had it to claim.
            return worktree_ready_reply(
                &github_repo,
                self.action_executor.is_some() && self.action_registry.is_some(),
            );
        }

        format!("Added repository {github_repo} to project")
    }
}

/// `add_project_repo`'s reply once the worktree is on disk.
///
/// `runs_setup_actions` is whether this handler has the action executor and
/// registry its detached setup task needs; without them that task logs and
/// skips, so the reply must not claim setup actions are in flight.
fn worktree_ready_reply(github_repo: &str, runs_setup_actions: bool) -> String {
    if runs_setup_actions {
        format!(
            "Added repository {github_repo} to project — the worktree is ready; setup actions are running in the background"
        )
    } else {
        format!("Added repository {github_repo} to project — the worktree is ready")
    }
}

#[tool_handler]
impl ServerHandler for ProjectToolsHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Start a local MCP SSE server for a project session.
///
/// Returns the bound port and a JoinHandle. The server runs until
/// the handle (and its parent LocalSet) is dropped.
#[allow(clippy::too_many_arguments)]
pub async fn start_project_mcp_server(
    project_id: String,
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
    action_executor: Option<Arc<ActionExecutor>>,
    action_registry: Option<Arc<ActionRegistry>>,
    provider: Option<String>,
    acp_config_selection: Option<AcpConfigSelection>,
    parent_project_note_id: Option<String>,
    cancel_token: CancellationToken,
) -> Result<(u16, JoinHandle<()>), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind MCP listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get local address: {e}"))?
        .port();

    let handler = ProjectToolsHandler::new(
        project_id,
        store,
        registry,
        app_handle,
        action_executor,
        action_registry,
        provider,
        acp_config_selection,
        parent_project_note_id,
        cancel_token,
    );
    log::debug!(
        "[project_mcp] HTTP server bound on port {port} for project {}",
        handler.project_id
    );

    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let router = Router::new().route_service("/mcp", service);

    let handle = tokio::task::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            log::error!("[project_mcp] HTTP server error: {e}");
        }
    });

    Ok((port, handle))
}

#[cfg(test)]
mod tests {
    use super::{
        build_repo_note_stub, comment_line_range, inherited_acp_config_selection,
        worktree_ready_reply, ProjectToolsHandler, RepoArtifactKind,
        REPO_SESSION_ACTIVITY_PREVIEW_MAX_CHARS,
    };
    use crate::git::Span;
    use crate::store::{
        AcpConfigSelection, AcpConfigValueSelection, Branch, MessageRole, Project, ProjectNote,
        Session, SessionMessage, Store,
    };
    use std::path::Path;

    #[test]
    fn comment_line_range_reports_one_indexed_inclusive_lines() {
        // Stored spans are 0-indexed with an exclusive end, so lines 11..=15
        // of the file are stored as 10..15.
        assert_eq!(comment_line_range(Span::new(10, 15)), (11, 15));
        // Single-line comment.
        assert_eq!(comment_line_range(Span::new(10, 11)), (11, 11));
        // Empty span: collapse onto the line it anchors to.
        assert_eq!(comment_line_range(Span::new(10, 10)), (11, 11));
        // First line of the file.
        assert_eq!(comment_line_range(Span::new(0, 1)), (1, 1));
    }

    #[test]
    fn inherited_acp_config_selection_follows_the_inherited_provider_only() {
        let selection = AcpConfigSelection {
            model: Some(AcpConfigValueSelection {
                config_id: "model".to_string(),
                value_id: "claude-opus-5".to_string(),
                label: None,
            }),
            effort: None,
        };

        // The session runs on the parent's own provider: selection inherited.
        assert_eq!(
            inherited_acp_config_selection(Some("claude"), Some("claude"), Some(&selection)),
            Some(selection.clone())
        );

        // Review provider resolution fell back to a different agent: the
        // parent's config/value IDs don't exist there, so no selection.
        assert_eq!(
            inherited_acp_config_selection(Some("codex"), Some("claude"), Some(&selection)),
            None
        );

        // No inherited provider to compare against (the review resolved the
        // preferred provider instead): the selection's provider is unknown,
        // so it is dropped rather than risked on the resolved agent.
        assert_eq!(
            inherited_acp_config_selection(None, Some("claude"), Some(&selection)),
            None
        );

        // Non-review outcomes pass the inherited provider through unchanged,
        // including when it is absent.
        assert_eq!(
            inherited_acp_config_selection(None, None, Some(&selection)),
            Some(selection)
        );

        assert_eq!(
            inherited_acp_config_selection(Some("claude"), Some("claude"), None),
            None
        );
    }

    #[test]
    fn worktree_ready_reply_only_promises_setup_actions_when_they_can_run() {
        let with_executor = worktree_ready_reply("block/staged", true);
        assert!(
            with_executor.contains("setup actions are running in the background"),
            "unexpected reply: {with_executor}"
        );

        let without_executor = worktree_ready_reply("block/staged", false);
        assert!(
            !without_executor.contains("setup actions"),
            "reply must not promise setup actions with no executor: {without_executor}"
        );
        assert!(
            without_executor.contains("block/staged") && without_executor.contains("worktree"),
            "unexpected reply: {without_executor}"
        );
    }

    #[test]
    fn child_note_stub_attaches_to_parent_and_hides_from_timeline() {
        let store = Store::in_memory().unwrap();
        let project = Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let parent = ProjectNote::new(&project.id, "Parent", "aggregated");
        store.create_project_note(&parent).unwrap();

        // A `child_note` outcome with a parent in scope attaches to that parent.
        let note = build_repo_note_stub(
            &branch.id,
            "investigate the auth module",
            "session-child",
            Some(&parent.id),
        );
        assert_eq!(
            note.parent_project_note_id.as_deref(),
            Some(parent.id.as_str())
        );
        store.create_note(&note).unwrap();

        // Returned by the dedicated parent-note query...
        let children = store.list_child_notes(&parent.id).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, note.id);
        // ...but excluded from the repo's visible timeline.
        assert!(store.list_notes_for_branch(&branch.id).unwrap().is_empty());
    }

    #[test]
    fn child_note_stub_without_parent_is_detached() {
        let store = Store::in_memory().unwrap();
        let project = Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();

        // No parent in scope (e.g. a non-project session): fall back to a plain
        // detached note that is visible in the timeline.
        let note = build_repo_note_stub(&branch.id, "write a note", "session-detached", None);
        assert!(note.parent_project_note_id.is_none());
        store.create_note(&note).unwrap();

        let timeline = store.list_notes_for_branch(&branch.id).unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].id, note.id);
        assert!(store.list_child_notes(&note.id).unwrap().is_empty());
    }

    #[test]
    fn repo_session_handles_round_trip() {
        let encoded = ProjectToolsHandler::encode_repo_session_handle(
            "session-123",
            RepoArtifactKind::Commit,
            "commit-456",
        )
        .expect("handle should encode");
        let decoded = ProjectToolsHandler::decode_repo_session_handle(&encoded)
            .expect("handle should decode");

        assert_eq!(decoded.session_id, "session-123");
        assert!(matches!(decoded.artifact_kind, RepoArtifactKind::Commit));
        assert_eq!(decoded.artifact_id, "commit-456");
    }

    #[test]
    fn review_repo_session_handles_round_trip() {
        let encoded = ProjectToolsHandler::encode_repo_session_handle(
            "session-123",
            RepoArtifactKind::Review,
            "review-789",
        )
        .expect("handle should encode");
        let decoded = ProjectToolsHandler::decode_repo_session_handle(&encoded)
            .expect("handle should decode");

        assert_eq!(decoded.session_id, "session-123");
        assert!(matches!(decoded.artifact_kind, RepoArtifactKind::Review));
        assert_eq!(decoded.artifact_id, "review-789");
    }

    #[test]
    fn repo_session_handles_reject_invalid_prefix() {
        let err = ProjectToolsHandler::decode_repo_session_handle("session-123")
            .expect_err("invalid handle should fail");
        assert!(err.contains("Invalid repo_session_id"));
    }

    #[test]
    fn repo_session_activity_summarizes_recent_messages() {
        let mut session = Session::new_running("make progress", Path::new("/tmp"));
        session.created_at = 100;
        session.updated_at = 105;

        let messages = vec![
            SessionMessage {
                id: 1,
                session_id: session.id.clone(),
                role: MessageRole::User,
                content: "start".to_string(),
                created_at: 106,
                image_ids: vec![],
                acp: Default::default(),
            },
            SessionMessage {
                id: 2,
                session_id: session.id.clone(),
                role: MessageRole::Assistant,
                content: "Working\nthrough   change".to_string(),
                created_at: 107,
                image_ids: vec![],
                acp: Default::default(),
            },
            SessionMessage {
                id: 3,
                session_id: session.id.clone(),
                role: MessageRole::ToolCall,
                content: "cargo test".to_string(),
                created_at: 108,
                image_ids: vec![],
                acp: Default::default(),
            },
            SessionMessage {
                id: 4,
                session_id: session.id.clone(),
                role: MessageRole::ToolResult,
                content: "tests still running\n123".to_string(),
                created_at: 109,
                image_ids: vec![],
                acp: Default::default(),
            },
        ];

        let activity = ProjectToolsHandler::summarize_session_activity(&session, &messages);

        assert_eq!(activity.last_activity_at, Some(109));
        assert_eq!(activity.counts.assistant_messages, 1);
        assert_eq!(activity.counts.tool_calls, 1);
        assert_eq!(activity.counts.tool_results, 1);

        let last_message = activity.last_message.expect("last message");
        assert_eq!(last_message.role, "tool_result");
        assert_eq!(last_message.created_at, 109);
        assert_eq!(last_message.preview, "tests still running 123");

        let last_tool_call = activity.last_tool_call.expect("last tool call");
        assert_eq!(last_tool_call.created_at, 108);
        assert_eq!(last_tool_call.preview, "cargo test");

        let last_tool_result = activity.last_tool_result.expect("last tool result");
        assert_eq!(last_tool_result.created_at, 109);
        assert_eq!(last_tool_result.preview, "tests still running 123");
    }

    #[test]
    fn repo_session_activity_uses_session_timestamp_without_messages() {
        let mut session = Session::new_running("queued work", Path::new("/tmp"));
        session.created_at = 100;
        session.updated_at = 150;

        let activity = ProjectToolsHandler::summarize_session_activity(&session, &[]);

        assert_eq!(activity.last_activity_at, Some(150));
        assert!(activity.last_message.is_none());
        assert!(activity.last_tool_call.is_none());
        assert!(activity.last_tool_result.is_none());
        assert_eq!(activity.counts.assistant_messages, 0);
        assert_eq!(activity.counts.tool_calls, 0);
        assert_eq!(activity.counts.tool_results, 0);
    }

    #[test]
    fn repo_session_activity_preview_is_bounded() {
        let content = "x".repeat(REPO_SESSION_ACTIVITY_PREVIEW_MAX_CHARS + 1);
        let preview = ProjectToolsHandler::activity_preview(&content);

        assert_eq!(
            preview.chars().count(),
            REPO_SESSION_ACTIVITY_PREVIEW_MAX_CHARS + 3
        );
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn start_repo_session_description_lists_all_outcomes() {
        let router = ProjectToolsHandler::tool_router();
        let start_description = router
            .get("start_repo_session")
            .and_then(|tool| tool.description.as_deref())
            .expect("start tool description");

        assert!(start_description.contains("note_in_repo"));
        assert!(start_description.contains("\"commit\""));
        assert!(start_description.contains("code_review"));
        assert!(start_description.contains("AI code review"));
    }

    #[test]
    fn start_repo_session_schema_lets_review_instructions_stay_empty() {
        let router = ProjectToolsHandler::tool_router();
        let instructions = router
            .get("start_repo_session")
            .and_then(|tool| {
                tool.input_schema
                    .get("properties")?
                    .get("instructions")?
                    .get("description")?
                    .as_str()
                    .map(ToOwned::to_owned)
            })
            .expect("instructions description");

        assert!(
            instructions.contains("code_review"),
            "instructions must explain the review outcome: {instructions}"
        );
        assert!(
            instructions.contains("leave this empty"),
            "instructions must say a standard review needs none: {instructions}"
        );
    }

    #[test]
    fn repo_session_tool_descriptions_avoid_field_names_and_explain_cancellation() {
        let router = ProjectToolsHandler::tool_router();
        let wait_description = router
            .get("wait_for_repo_session")
            .and_then(|tool| tool.description.as_deref())
            .expect("wait tool description");
        let cancel_description = router
            .get("cancel_repo_session")
            .and_then(|tool| tool.description.as_deref())
            .expect("cancel tool description");

        assert!(wait_description.contains("Returns the current state"));
        assert!(!wait_description.contains("`activity` progress fields"));
        assert!(!wait_description.contains("`last_activity_at`"));
        assert!(!wait_description.contains("`last_tool_result`"));

        assert!(cancel_description.contains("Abort a repo session"));
        assert!(cancel_description.contains("when the user wants the session stopped"));
        assert!(cancel_description.contains("go down a different path"));
        assert!(cancel_description.contains("surprised at how long the session is taking"));
        assert!(!cancel_description.contains("strong evidence"));
        assert!(!cancel_description.contains("taking a long time"));
    }
}
