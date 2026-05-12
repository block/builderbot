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
use tauri::{AppHandle, Emitter};

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::session_runner::SessionRegistry;
use crate::store::{Branch, ProjectRepo, SessionStatus, Store};
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
    pub instructions: String,
    /// What the session should produce. Controls the prompt given to the agent and what
    /// artifact (if any) is created in the database.
    ///
    /// - `"note_in_repo"`: Use this for generating notes that can be referred to again
    ///   later by other sessions or by the user. Useful for architecture overviews, plans,
    ///   research, reviews.
    /// - `"commit"`: Use this to request code changes. Agent makes code changes and
    ///   creates a signed-off commit with a conventional commit message.
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

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum RepoArtifactKind {
    Note,
    Commit,
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
                },
                "id": handle.artifact_id,
            },
        });

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
            }

            payload["output"] =
                serde_json::Value::String(self.last_assistant_output(&handle.session_id));
        }

        Ok(payload)
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
            cancel_token,
        }
    }
}

#[tool_router]
impl ProjectToolsHandler {
    #[tool(
        description = "Enqueue an agent session in one of the project's repositories and return immediately with an opaque `repo_session_id`. Use `expected_outcome=\"note_in_repo\"` for repo notes or `expected_outcome=\"commit\"` for code changes and a signed-off conventional commit. The `repo` + `subpath` combination must exactly match an entry already in the project."
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

        let mut session = crate::store::Session::new_queued(&p.instructions);
        if let Some(ref provider) = self.provider {
            session = session.with_provider(provider);
        }
        if let Err(e) = self.store.create_session(&session) {
            return format!("Error creating queued session: {e}");
        }

        let (artifact_id, artifact_kind) = match p.expected_outcome {
            RepoSessionOutcome::NoteInRepo => {
                let note = crate::store::Note::new(&target.branch.id, &p.instructions, "")
                    .with_session(&session.id);
                let note_id = note.id.clone();
                if let Err(e) = self.store.create_note(&note) {
                    return format!("Error creating note stub: {e}");
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
        };

        let repo_session_id =
            match Self::encode_repo_session_handle(&session.id, artifact_kind, &artifact_id) {
                Ok(id) => id,
                Err(e) => return e,
            };

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
        description = "Wait for a repo session started by `start_repo_session`. Returns the queued/running/completed/cancelled/failed state for the opaque `repo_session_id`. `wait_for_completion_seconds` defaults to 240."
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
        description = "Cancel a repo session started by `start_repo_session`. Cancels either the queued item or the running session behind the opaque `repo_session_id`."
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
            Some(&crate::store::CompletionReason::Interrupted),
        ) {
            Ok(updated) => updated,
            Err(e) => return format!("Error cancelling repo session: {e}"),
        };

        if was_queued {
            // Successfully cancelled from queued state. Drain the next
            // queued session for this branch so it can start.
            if let Ok(Some(branch_id)) = self.store.get_branch_id_for_session(&handle.session_id) {
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
            self.registry.cancel(&handle.session_id);
        }

        match self.repo_session_payload(&p.repo_session_id, &handle) {
            Ok(payload) => payload.to_string(),
            Err(e) => e,
        }
    }

    #[tool(
        description = "Add a GitHub repository to the current project. Use this when the task requires a repository that isn't yet in the project. Waits until the repository worktree is ready and setup actions have completed before returning."
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
        let _ = self
            .app_handle
            .emit("project-setup-progress", self.project_id.clone());

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
                    let _ = self
                        .app_handle
                        .emit("project-setup-progress", self.project_id.clone());
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

            // Run detect_actions + prerun actions if we have an executor
            if let (Some(executor), Some(act_registry)) =
                (self.action_executor.as_ref(), self.action_registry.as_ref())
            {
                // Atomically claim setup ownership before running prerun actions.
                match self.store.mark_branch_setup_complete(&branch.id) {
                    Ok(true) => {
                        log::debug!(
                            "[project_mcp] add_project_repo: running prerun actions for branch {}",
                            branch.id
                        );
                        match crate::branches::run_prerun_actions_for_branch(
                            &self.store,
                            &self.app_handle,
                            &branch.id,
                            executor,
                            act_registry,
                        )
                        .await
                        {
                            Ok(count) => {
                                log::debug!(
                                    "[project_mcp] add_project_repo: ran {count} prerun actions for branch {}",
                                    branch.id
                                );
                                // Notify UI that prerun actions finished
                                let _ = self
                                    .app_handle
                                    .emit("project-setup-progress", self.project_id.clone());
                            }
                            Err(e) => {
                                log::warn!(
                                    "[project_mcp] add_project_repo: prerun actions failed (continuing): {e}"
                                );
                            }
                        }
                    }
                    Ok(false) => {
                        log::debug!(
                            "[project_mcp] add_project_repo: branch {} already setup complete, skipping prerun",
                            branch.id
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[project_mcp] add_project_repo: failed to mark setup complete: {e}"
                        );
                    }
                }
            } else {
                log::debug!(
                    "[project_mcp] add_project_repo: no action executor available, skipping prerun actions"
                );
            }

            // If the repo already has commits on this branch, kick off
            // an automatic code review so the user gets immediate feedback.
            crate::maybe_trigger_auto_review_for_new_repo(
                &self.store,
                &self.app_handle,
                &branch.id,
                Some(&worktree_path),
            )
            .await;
        }

        format!("Added repository {github_repo} to project")
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
    use super::{ProjectToolsHandler, RepoArtifactKind};

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
    fn repo_session_handles_reject_invalid_prefix() {
        let err = ProjectToolsHandler::decode_repo_session_handle("session-123")
            .expect_err("invalid handle should fail");
        assert!(err.contains("Invalid repo_session_id"));
    }
}
