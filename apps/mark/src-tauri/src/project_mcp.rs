//! MCP server for project sessions.
//! Exposes `start_repo_session` and `add_project_repo` tools to the agent.

use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use axum::Router;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use tauri::{AppHandle, Emitter};

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::session_runner::{SessionConfig, SessionRegistry};
use crate::store::{Session, SessionStatus, Store};
use tokio_util::sync::CancellationToken;

/// What outcome the caller expects from a `start_repo_session` call.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RepoSessionOutcome {
    /// The session should only return output to the caller. No artifact is created.
    ReturnOutputOnly,
    /// The session should produce a note in the repository. A note stub is created and
    /// the agent is instructed to output note content after a horizontal rule (---).
    NoteInRepo,
    /// The session should make code changes and create a commit. A pending commit record
    /// is created and the agent is instructed to commit with a conventional commit message.
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
    /// - `"return_output_only"`: Agent returns output only; use `return_info` to
    ///   specify exactly what you want back.
    /// - `"note_in_repo"`: Use this for generating notes that can be referred to again
    ///   later by other sessions or by the user. Useful for architecture overviews, plans,
    ///   research, reviews.
    /// - `"commit"`: Use this to request code changes. Agent makes code changes and
    ///   creates a commit with a conventional commit message.
    pub expected_outcome: RepoSessionOutcome,
    /// Only used when `expected_outcome` is `"return_output_only"`.
    /// Describe what information you want the session to return to you when it finishes.
    /// Example: "a summary of all changes made and any errors encountered".
    pub return_info: Option<String>,
    /// Optional ACP provider ID (e.g. "claude", "goose").
    pub provider: Option<String>,
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
    /// Cancellation token for the parent project session.
    /// Signalled when the user cancels the project session.
    cancel_token: CancellationToken,
    /// Optional workspace name for the parent project session.
    /// When `Some`, notes are written to temp files inside the remote workspace
    /// via `ws_exec`. When `None`, notes are written to local temp files.
    workspace_name: Option<String>,
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
        cancel_token: CancellationToken,
        workspace_name: Option<String>,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            project_id,
            store,
            registry,
            app_handle,
            action_executor,
            action_registry,
            cancel_token,
            workspace_name,
        }
    }
}

#[tool_router]
impl ProjectToolsHandler {
    #[tool(
        description = "Start an agent session in one of the project's repositories. Waits for completion and returns the outcome. Use `expected_outcome` to control what the session produces: `\"return_output_only\"` (use `return_info` to describe what to return), `\"note_in_repo\"` (agent researches and writes a note visible in the branch card), or `\"commit\"` (agent makes code changes and creates a commit visible in the branch card). The `repo` + `subpath` combination must exactly match an entry already in the project — call will fail immediately otherwise."
    )]
    async fn start_repo_session(
        &self,
        Parameters(p): Parameters<StartRepoSessionParams>,
    ) -> String {
        log::info!(
            "[project_mcp] start_repo_session called: repo={:?} subpath={:?} expected_outcome={:?} return_info={:?} provider={:?} instructions={:?}",
            p.repo,
            p.subpath,
            p.expected_outcome,
            p.return_info,
            p.provider,
            p.instructions,
        );
        // Find the matching project repo — must match both github_repo and subpath exactly.
        let repos = match self.store.list_project_repos(&self.project_id) {
            Ok(r) => r,
            Err(e) => return format!("Error listing repos: {e}"),
        };
        let repo = match repos
            .iter()
            .find(|r| r.github_repo == p.repo && r.subpath.as_deref() == p.subpath.as_deref())
        {
            Some(r) => r.clone(),
            None => {
                let available = repos
                    .iter()
                    .map(|r| match r.subpath.as_deref() {
                        Some(sp) => format!("{} (subpath: {sp})", r.github_repo),
                        None => r.github_repo.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return format!(
                    "Repository '{}' with subpath {:?} not found in project. Available repos: {}",
                    p.repo, p.subpath, available
                );
            }
        };

        // Find the branch for this repo — capture the full struct for context building.
        let branches = match self.store.list_branches_for_project(&self.project_id) {
            Ok(b) => b,
            Err(e) => return format!("Error listing branches: {e}"),
        };
        let branch = branches
            .into_iter()
            .find(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str()));
        let workspace_name = branch.as_ref().and_then(|b| b.workspace_name.clone());
        let branch_id = branch.as_ref().map(|b| b.id.clone());

        // Determine working directory — include subpath when the repo was added with one.
        // For local branches, use the project worktree path so the agent operates on the
        // correct branch; error if no worktree is recorded (repo not fully set up yet).
        let clone_dir = crate::paths::repos_dir()
            .map(|d| {
                let base = d.join(&repo.github_repo);
                if let Some(ref sp) = repo.subpath {
                    base.join(sp)
                } else {
                    base
                }
            })
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let working_dir = if workspace_name.is_none() {
            match branch.as_ref().and_then(|br| {
                self.store
                    .get_workdir_for_branch(&br.id)
                    .ok()
                    .flatten()
                    .map(|wd| {
                        let mut path = std::path::PathBuf::from(&wd.path);
                        if let Some(ref sp) = repo.subpath {
                            path = path.join(sp);
                        }
                        path
                    })
            }) {
                Some(path) => path,
                None => return format!(
                    "No worktree found for repo '{}'. Ensure the repo has been fully set up via add_project_repo before starting a session.",
                    repo.github_repo
                ),
            }
        } else {
            clone_dir
        };

        // Build branch history context (commits + notes) and project context, mirroring
        // what start_branch_session does for user-triggered sessions.
        let project = self.store.get_project(&self.project_id).ok().flatten();
        let (branch_context, project_information) = if let Some(ref br) = branch {
            let store_clone = Arc::clone(&self.store);
            let branch_id_str = br.id.clone();
            let base_branch = br.base_branch.clone();
            let project_id_str = self.project_id.clone();
            let ws_name = workspace_name.clone();

            // working_dir is already the worktree path for local branches.
            let context_dir = working_dir.clone();

            let ctx = tokio::task::spawn_blocking(move || {
                if let Some(ws) = ws_name {
                    crate::session_commands::build_remote_branch_context(
                        &ws,
                        &base_branch,
                        &store_clone,
                        &branch_id_str,
                        &project_id_str,
                    )
                } else {
                    crate::session_commands::build_branch_context(
                        &context_dir,
                        &base_branch,
                        &store_clone,
                        &branch_id_str,
                        &project_id_str,
                    )
                }
            })
            .await
            .unwrap_or_else(|e| {
                log::warn!("[project_mcp] context build task panicked: {e}");
                String::new()
            });

            let proj_info = if let (Some(proj), Some(br_ref)) = (project.as_ref(), Some(br)) {
                crate::session_commands::build_project_context(&self.store, proj, br_ref)
            } else {
                String::new()
            };

            (ctx, proj_info)
        } else {
            (String::new(), String::new())
        };

        // Build the prompt — action instructions + project info + branch history + user instructions,
        // matching the structure produced by build_full_prompt for user-triggered sessions.
        let expected_outcome = &p.expected_outcome;
        let action_instructions = match expected_outcome {
            RepoSessionOutcome::ReturnOutputOnly => None,
            RepoSessionOutcome::NoteInRepo => Some(
                "The user is requesting a note. Generate a note based on their instructions below.\n\n\
                 You may use any tools needed to research and gather information, but do NOT create \
                 any commits.\n\n\
                 To return the note, include a horizontal rule (---) followed by the note content. \
                 Begin the note with a markdown H1 heading as the title."
            ),
            RepoSessionOutcome::Commit => Some(
                "The user is requesting you make a commit based on the instructions below. Make the necessary \
                 code changes, following any verification or formatting steps as instructed, and then \
                 create a commit with a conventional commit message. This commit should describe what \
                 was requested and how it was fulfilled."
            ),
        };
        let user_instructions = match (expected_outcome, p.return_info.as_ref()) {
            (RepoSessionOutcome::ReturnOutputOnly, Some(return_info)) => format!(
                "{}\n\nIMPORTANT: When you are done, your final message must contain: {return_info}",
                p.instructions
            ),
            _ => p.instructions.clone(),
        };
        let prompt = {
            let action_block = match action_instructions {
                Some(instr) if !project_information.is_empty() => format!(
                    "<action>\n{instr}\n\nProject information:\n{project_information}\n</action>"
                ),
                Some(instr) => format!("<action>\n{instr}\n</action>"),
                None if !project_information.is_empty() => {
                    format!("<action>\nProject information:\n{project_information}\n</action>")
                }
                None => String::new(),
            };
            let history_block = if !branch_context.is_empty() {
                format!("<branch-history>\n{branch_context}\n</branch-history>")
            } else {
                String::new()
            };
            [action_block, history_block, user_instructions]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        };

        // Create the session record.
        let mut session = Session::new_running(&prompt, &working_dir);
        if let Some(ref prov) = p.provider {
            session = session.with_provider(prov);
        }
        if let Err(e) = self.store.create_session(&session) {
            return format!("Error creating session: {e}");
        }
        let session_id = session.id.clone();

        // Create artifact stub and capture pre_head_sha (for commit sessions).
        let (artifact_id, pre_head_sha) = match expected_outcome {
            RepoSessionOutcome::NoteInRepo => match branch_id.as_deref() {
                Some(bid) => {
                    let note = crate::store::Note::new(bid, "", "").with_session(&session_id);
                    let note_id = note.id.clone();
                    if let Err(e) = self.store.create_note(&note) {
                        log::error!("[project_mcp] failed to create note stub: {e}");
                    }
                    (Some(note_id), None)
                }
                None => {
                    log::warn!(
                            "[project_mcp] expected_outcome=note_in_repo but no branch found for repo {}",
                            repo.github_repo
                        );
                    (None, None)
                }
            },
            RepoSessionOutcome::Commit => {
                match branch_id.as_deref() {
                    Some(bid) => {
                        let commit =
                            crate::store::Commit::new_pending(bid).with_session(&session_id);
                        let commit_id = commit.id.clone();
                        if let Err(e) = self.store.create_commit(&commit) {
                            log::error!("[project_mcp] failed to create commit stub: {e}");
                        }
                        // Capture HEAD SHA before the session runs so post-completion
                        // hooks can detect whether a new commit was created.
                        let wd = working_dir.clone();
                        let ws = workspace_name.clone();
                        let sha = if let Some(ws_name) = ws {
                            tokio::task::spawn_blocking(move || {
                                crate::blox::ws_exec(&ws_name, &["git", "rev-parse", "HEAD"])
                                    .map(|s| s.trim().to_string())
                            })
                            .await
                            .ok()
                            .and_then(|r| r.ok())
                        } else {
                            crate::git::get_head_sha(&wd).ok()
                        };
                        (Some(commit_id), sha)
                    }
                    None => {
                        log::warn!(
                            "[project_mcp] expected_outcome=commit but no branch found for repo {}",
                            repo.github_repo
                        );
                        (None, None)
                    }
                }
            }
            RepoSessionOutcome::ReturnOutputOnly => (None, None),
        };

        // Start the agent (returns immediately; work happens on background thread).
        let start_result = crate::session_runner::start_session(
            SessionConfig {
                session_id: session_id.clone(),
                prompt,
                working_dir,
                agent_session_id: None,
                pre_head_sha,
                provider: p.provider,
                workspace_name,
                extra_env: vec![],
                mcp_project_id: None,
                action_executor: None,
                action_registry: None,
            },
            Arc::clone(&self.store),
            self.app_handle.clone(),
            Arc::clone(&self.registry),
        );
        if let Err(e) = start_result {
            return format!("Error starting session: {e}");
        }

        // Notify the frontend that a new session is running in this branch so it
        // can register the session in its state stores and refresh the branch card
        // timeline immediately (same pattern as `project-repo-added`).
        if let Some(ref bid) = branch_id {
            let session_type = match expected_outcome {
                RepoSessionOutcome::NoteInRepo => Some("note"),
                RepoSessionOutcome::Commit => Some("commit"),
                RepoSessionOutcome::ReturnOutputOnly => None,
            };
            if let Some(stype) = session_type {
                crate::session_runner::emit_session_running(
                    &self.app_handle,
                    &session_id,
                    bid,
                    &self.project_id,
                    stype,
                );
            }
        }

        // Poll until the session reaches a terminal state.
        // Also watch the parent project session's cancellation token so we
        // don't loop forever if the project session is cancelled while waiting.
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = self.cancel_token.cancelled() => {
                    // Cancel the child session so it doesn't run as an orphan
                    // after the parent project session has been cancelled.
                    self.registry.cancel(&session_id);
                    return serde_json::json!({
                        "session_id": session_id,
                        "outcome": "cancelled",
                        "output": "",
                    })
                    .to_string();
                }
            }
            match self.store.get_session(&session_id) {
                Ok(Some(s)) if s.status != SessionStatus::Running => {
                    let outcome = match s.status {
                        SessionStatus::Completed => "completed",
                        SessionStatus::Cancelled => "cancelled",
                        _ => "failed",
                    };
                    // Return the last assistant message as the session output so the
                    // parent agent receives the result the child was asked to produce.
                    let output = self
                        .store
                        .get_session_messages(&session_id)
                        .ok()
                        .and_then(|msgs| {
                            msgs.into_iter()
                                .rfind(|m| m.role == crate::store::MessageRole::Assistant)
                                .map(|m| m.content)
                        })
                        .unwrap_or_default();
                    // For note sessions, strip the note content (everything from the
                    // first --- separator onwards) — it's provided separately in `note`.
                    let output = if matches!(expected_outcome, RepoSessionOutcome::NoteInRepo) {
                        let mut sep_line = None;
                        for (i, line) in output.lines().enumerate() {
                            let t = line.trim();
                            if t == "---" || t == "***" || t == "___" {
                                sep_line = Some(i);
                                break;
                            }
                        }
                        match sep_line {
                            Some(i) => output.lines().take(i).collect::<Vec<_>>().join("\n"),
                            None => output,
                        }
                    } else {
                        output
                    };
                    // If this was a note session, include the note info in the same format
                    // provided at session start for available notes.
                    let note_info: Option<String> =
                        if matches!(expected_outcome, RepoSessionOutcome::NoteInRepo) {
                            artifact_id.as_deref().and_then(|note_id| {
                                match self.store.get_note(note_id) {
                                    Ok(Some(note)) if !note.content.is_empty() => {
                                        crate::session_commands::format_note_for_context(
                                            &note.id,
                                            &note.title,
                                            &note.content,
                                            self.workspace_name.as_deref(),
                                        )
                                    }
                                    _ => None,
                                }
                            })
                        } else {
                            None
                        };
                    let mut result = serde_json::json!({
                        "session_id": session_id,
                        "outcome": outcome,
                        "output": output,
                    });
                    if let Some(aid) = artifact_id.as_deref() {
                        result["artifact_id"] = serde_json::Value::String(aid.to_string());
                    }
                    if let Some(note) = note_info {
                        result["note"] = serde_json::Value::String(note);
                    }
                    return result.to_string();
                }
                Ok(Some(_)) => continue, // still running
                Ok(None) => return format!("Session {session_id} was deleted while running"),
                Err(e) => return format!("Error polling session status: {e}"),
            }
        }
    }

    #[tool(
        description = "Add a GitHub repository to the current project. Use this when the task requires a repository that isn't yet in the project. Waits until the repository worktree is ready and setup actions have completed before returning."
    )]
    async fn add_project_repo(&self, Parameters(p): Parameters<AddProjectRepoParams>) -> String {
        log::info!(
            "[project_mcp] add_project_repo called: github_repo={:?} branch_name={:?} subpath={:?}",
            p.github_repo,
            p.branch_name,
            p.subpath,
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

        let github_repo = p.github_repo.clone();
        let repo = match crate::project_commands::add_project_repo_impl(
            Arc::clone(&self.store),
            self.project_id.clone(),
            p.github_repo,
            p.branch_name,
            p.subpath,
            None,
            p.reason,
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
                return format!(
                    r#"{{"repo_id": "{}", "message": "Added repository {} to project (no branch found)"}}"#,
                    repo.id, github_repo
                );
            }
        };

        // For local branches, set up the git worktree
        if branch.workspace_name.is_none() {
            log::info!(
                "[project_mcp] add_project_repo: setting up worktree for branch {}",
                branch.branch_name
            );
            let branch_id = branch.id.clone();
            let store = Arc::clone(&self.store);
            let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                // We need to run the worktree setup synchronously
                // Reuse the core logic from branches::setup_worktree
                crate::branches::setup_worktree_sync(&store, &branch_id)
            })
            .await;

            match worktree_result {
                Ok(Ok(worktree_path)) => {
                    log::info!(
                        "[project_mcp] add_project_repo: worktree ready at {}",
                        worktree_path
                    );
                    // Notify UI that the worktree is ready so branch state updates
                    let _ = self
                        .app_handle
                        .emit("project-setup-progress", self.project_id.clone());
                }
                Ok(Err(e)) => {
                    log::warn!(
                        "[project_mcp] add_project_repo: worktree setup failed (continuing): {e}"
                    );
                    // Don't abort — return the repo even if worktree setup failed
                    return serde_json::json!({
                        "repo_id": repo.id,
                        "message": format!("Added repository {github_repo} to project (worktree setup failed: {e})"),
                    })
                    .to_string();
                }
                Err(e) => {
                    log::warn!(
                        "[project_mcp] add_project_repo: worktree task panicked (continuing): {e}"
                    );
                    return serde_json::json!({
                        "repo_id": repo.id,
                        "message": format!("Added repository {github_repo} to project (worktree task error: {e})"),
                    })
                    .to_string();
                }
            }

            // Run detect_actions + prerun actions if we have an executor
            if let (Some(executor), Some(act_registry)) =
                (self.action_executor.as_ref(), self.action_registry.as_ref())
            {
                log::info!(
                    "[project_mcp] add_project_repo: running prerun actions for branch {}",
                    branch.id
                );
                let prerun_result = crate::branches::run_prerun_actions_for_branch(
                    &self.store,
                    &self.app_handle,
                    &branch.id,
                    executor,
                    act_registry,
                )
                .await;
                match prerun_result {
                    Ok(count) => {
                        log::info!(
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
            } else {
                log::info!(
                    "[project_mcp] add_project_repo: no action executor available, skipping prerun actions"
                );
            }
        }

        serde_json::json!({
            "repo_id": repo.id,
            "message": format!("Added repository {github_repo} to project"),
        })
        .to_string()
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
    cancel_token: CancellationToken,
    workspace_name: Option<String>,
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
        cancel_token,
        workspace_name,
    );
    log::info!(
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
