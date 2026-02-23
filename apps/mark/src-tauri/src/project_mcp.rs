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

use crate::actions::events::TauriExecutionListener;
use crate::actions::{ActionExecutor, ActionMetadata, ActionRegistry, ActionType};
use crate::session_runner::{SessionConfig, SessionRegistry};
use crate::store::{Session, SessionStatus, Store};

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
    /// Instructions to give the agent.
    pub instructions: String,
    /// What the session should produce. Controls the prompt given to the agent and what
    /// artifact (if any) is created in the database.
    ///
    /// - `"return_output_only"`: Agent returns output only; use `return_info` to
    ///   specify exactly what you want back.
    /// - `"note_in_repo"`: Agent researches and produces a note. Instructs the agent to
    ///   output content after a `---` horizontal rule with an H1 title.
    /// - `"commit"`: Agent makes code changes and creates a commit with a conventional
    ///   commit message.
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
    /// in the branch card timeline so they understand why it was added.
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
}

impl ProjectToolsHandler {
    fn new(
        project_id: String,
        store: Arc<Store>,
        registry: Arc<SessionRegistry>,
        app_handle: AppHandle,
        action_executor: Option<Arc<ActionExecutor>>,
        action_registry: Option<Arc<ActionRegistry>>,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            project_id,
            store,
            registry,
            app_handle,
            action_executor,
            action_registry,
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
        let repo = match repos.iter().find(|r| {
            (r.github_repo == p.repo || r.github_repo.ends_with(&format!("/{}", p.repo)))
                && r.subpath.as_deref() == p.subpath.as_deref()
        }) {
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

        // Find the branch for this repo — capture both workspace_name and branch_id.
        let branches = match self.store.list_branches_for_project(&self.project_id) {
            Ok(b) => b,
            Err(e) => return format!("Error listing branches: {e}"),
        };
        let branch = branches
            .iter()
            .find(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str()));
        let workspace_name = branch.and_then(|b| b.workspace_name.clone());
        let branch_id = branch.map(|b| b.id.clone());

        // Determine working directory — include subpath when the repo was added with one.
        let working_dir = crate::paths::repos_dir()
            .map(|d| {
                let base = d.join(&repo.github_repo);
                if let Some(ref sp) = repo.subpath {
                    base.join(sp)
                } else {
                    base
                }
            })
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

        // Build the prompt — prefix with action instructions based on expected outcome.
        let expected_outcome = &p.expected_outcome;
        let action_prefix = match expected_outcome {
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
        let prompt = {
            let base = match action_prefix {
                Some(prefix) => format!("{prefix}\n\n{}", p.instructions),
                None => p.instructions.clone(),
            };
            // For output-only sessions, append return_info instructions if provided.
            match (expected_outcome, p.return_info.as_ref()) {
                (RepoSessionOutcome::ReturnOutputOnly, Some(return_info)) => {
                    format!(
                        "{base}\n\nIMPORTANT: When you are done, your final message must contain: {return_info}"
                    )
                }
                _ => base,
            }
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
                    let note =
                        crate::store::Note::new(bid, &p.instructions, "").with_session(&session_id);
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

        // Poll until the session reaches a terminal state.
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
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
                                .filter(|m| m.role == crate::store::MessageRole::Assistant)
                                .last()
                                .map(|m| m.content)
                        })
                        .unwrap_or_default();
                    let output_json = serde_json::to_string(&output).unwrap_or_default();
                    let artifact_field = match artifact_id.as_deref() {
                        Some(aid) => format!(r#", "artifact_id": "{aid}""#),
                        None => String::new(),
                    };
                    return format!(
                        r#"{{"session_id": "{session_id}", "outcome": "{outcome}", "output": {output_json}{artifact_field}}}"#
                    );
                }
                Ok(_) => continue,
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
                Ok(Ok(score)) if score >= 20 => {
                    return format!(
                        "Error: '{}' appears to be a monorepo (score: {}). \
                         You must provide a `subpath` pointing to the root of the specific \
                         service or package you want to add (e.g. \"packages/api\" or \
                         \"services/auth\"). Re-call this tool with the appropriate subpath.",
                        p.github_repo, score
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
                Ok(Ok(_)) => {} // score < 20, not a monorepo
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
            .emit("project-repo-added", self.project_id.clone());

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
                setup_worktree_sync(&store, &branch_id)
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
                        .emit("project-repo-added", self.project_id.clone());
                }
                Ok(Err(e)) => {
                    log::warn!(
                        "[project_mcp] add_project_repo: worktree setup failed (continuing): {e}"
                    );
                    // Don't abort — return the repo even if worktree setup failed
                    return format!(
                        r#"{{"repo_id": "{}", "message": "Added repository {} to project (worktree setup failed: {})"}}"#,
                        repo.id, github_repo, e
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[project_mcp] add_project_repo: worktree task panicked (continuing): {e}"
                    );
                    return format!(
                        r#"{{"repo_id": "{}", "message": "Added repository {} to project (worktree task error: {})"}}"#,
                        repo.id, github_repo, e
                    );
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
                let prerun_result = run_prerun_actions_for_branch(
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
                            .emit("project-repo-added", self.project_id.clone());
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

        format!(
            r#"{{"repo_id": "{}", "message": "Added repository {} to project"}}"#,
            repo.id, github_repo
        )
    }
}

/// Set up a git worktree for a branch synchronously.
///
/// This replicates the core logic from `branches::setup_worktree` without
/// requiring Tauri state, so it can be called from the MCP server.
fn setup_worktree_sync(store: &Arc<Store>, branch_id: &str) -> Result<String, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    // Idempotent fast-path: if the branch already has a workdir, reuse it.
    if let Some(existing) = store
        .get_workdir_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
    {
        return Ok(existing.path);
    }

    // Resolve the repo slug for this branch
    let repo_slug = crate::branches::resolve_branch_repo_slug(store, &project, &branch)?;
    let repo_path = crate::git::ensure_local_clone(&repo_slug).map_err(|e| e.to_string())?;
    let desired_worktree_path =
        crate::git::project_worktree_path_for(&branch.project_id, &repo_slug, &branch.branch_name)
            .map_err(|e| e.to_string())?;

    // Reuse any existing worktree for this branch; otherwise create one.
    let existing_worktree_path = crate::git::list_worktrees(&repo_path)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find_map(|(path, wt_branch)| match wt_branch.as_deref() {
            Some(name) if name == branch.branch_name => Some(path),
            _ => None,
        });

    let worktree_path = if let Some(path) = existing_worktree_path {
        path
    } else if crate::git::branch_exists(&repo_path, &branch.branch_name)
        .map_err(|e| e.to_string())?
    {
        crate::git::create_worktree_for_existing_branch_at_path(
            &repo_path,
            &branch.branch_name,
            &desired_worktree_path,
        )
        .map_err(|e| e.to_string())?
    } else {
        match crate::git::create_worktree_at_path(
            &repo_path,
            &branch.branch_name,
            &branch.base_branch,
            &desired_worktree_path,
        ) {
            Ok(path) => path,
            Err(create_err) => {
                if crate::git::branch_exists(&repo_path, &branch.branch_name)
                    .map_err(|e| e.to_string())?
                {
                    log::warn!(
                        "[project_mcp] Branch '{}' already exists after create attempt; retrying with existing branch",
                        branch.branch_name
                    );
                    crate::git::create_worktree_for_existing_branch_at_path(
                        &repo_path,
                        &branch.branch_name,
                        &desired_worktree_path,
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    return Err(create_err.to_string());
                }
            }
        }
    };

    let worktree_str = worktree_path
        .to_str()
        .ok_or("Invalid worktree path")?
        .to_string();

    // Link this path to the branch in DB (create or assign existing record).
    let tracked_workdir = store
        .list_workdirs_for_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|wd| wd.path == worktree_str);

    match tracked_workdir {
        Some(wd) => match wd.branch_id.as_deref() {
            Some(existing_branch_id) if existing_branch_id != branch.id => {
                return Err(format!(
                    "Worktree '{}' is already assigned to another branch",
                    wd.path
                ));
            }
            Some(_) => {}
            None => {
                store
                    .assign_workdir(&wd.id, &branch.id)
                    .map_err(|e| e.to_string())?;
            }
        },
        None => {
            let workdir = crate::store::Workdir::new(&branch.project_id, &worktree_str)
                .with_branch(&branch.id);
            store.create_workdir(&workdir).map_err(|e| e.to_string())?;
        }
    }

    Ok(worktree_str)
}

/// Run detect_actions (if needed) and all prerun actions for a branch.
///
/// This replicates the core logic from `actions::commands::run_prerun_actions`
/// without requiring Tauri state.
async fn run_prerun_actions_for_branch(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    branch_id: &str,
    executor: &Arc<ActionExecutor>,
    act_registry: &Arc<ActionRegistry>,
) -> Result<usize, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Branch not found".to_string())?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Project not found".to_string())?;

    // Resolve the repo/subpath for this branch
    let (github_repo, subpath) = if let Some(project_repo_id) = &branch.project_repo_id {
        let project_repo = store
            .get_project_repo(project_repo_id)
            .map_err(|e| format!("Failed to get project repo: {e}"))?
            .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
        (project_repo.github_repo, project_repo.subpath)
    } else {
        let repo = project
            .primary_repo()
            .ok_or_else(|| "Project has no repository attached".to_string())?;
        (repo.to_string(), project.subpath.clone())
    };

    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;

    // If actions haven't been detected yet for this repo+subpath, detect now
    if !context.has_detected_actions {
        log::info!(
            "[project_mcp] detecting actions for repo {} (subpath: {:?})",
            github_repo,
            subpath
        );
        store
            .set_action_context_detecting(&context.id, true)
            .map_err(|e| format!("Failed to set detection status: {e}"))?;

        let _ = app_handle.emit(
            "repo-actions-detection",
            serde_json::json!({
                "githubRepo": github_repo,
                "subpath": subpath,
                "detecting": true,
            }),
        );

        // Run detection (may call out to AI)
        let detected = crate::actions::commands::detect_actions_for_repo_context(
            &github_repo,
            subpath.as_deref(),
        )
        .await
        .unwrap_or_default();

        // Persist detected actions (skip duplicates)
        let existing_actions = store
            .list_repo_actions(&context.id)
            .map_err(|e| format!("Failed to list actions: {e}"))?;
        let mut existing_commands: std::collections::HashSet<String> =
            existing_actions.iter().map(|a| a.command.clone()).collect();
        let mut next_sort_order = existing_actions
            .iter()
            .map(|a| a.sort_order)
            .max()
            .unwrap_or(-1)
            + 1;

        for suggestion in detected {
            if existing_commands.contains(&suggestion.command) {
                continue;
            }
            existing_commands.insert(suggestion.command.clone());
            let action = crate::store::RepoAction::new(
                context.id.clone(),
                suggestion.name,
                suggestion.command,
                suggestion.action_type,
                next_sort_order,
            )
            .with_auto_commit(suggestion.auto_commit);
            store
                .create_repo_action(&action)
                .map_err(|e| format!("Failed to create detected action: {e}"))?;
            next_sort_order += 1;
        }

        store
            .mark_action_context_detected(&context.id)
            .map_err(|e| format!("Failed to update detection status: {e}"))?;

        let _ = app_handle.emit(
            "repo-actions-detection",
            serde_json::json!({
                "githubRepo": github_repo,
                "subpath": subpath,
                "detecting": false,
            }),
        );
    }

    // Get all prerun actions for this context
    let actions = store
        .list_repo_actions(&context.id)
        .map_err(|e| format!("Failed to list actions: {e}"))?;
    let prerun_actions: Vec<_> = actions
        .into_iter()
        .filter(|a| matches!(a.action_type, ActionType::Prerun))
        .collect();

    if prerun_actions.is_empty() {
        return Ok(0);
    }

    // Get the worktree path for this branch
    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| format!("Failed to get workdir: {e}"))?
        .ok_or_else(|| "No worktree found for branch".to_string())?;

    let working_dir = if let Some(ref sp) = subpath {
        std::path::PathBuf::from(&workdir.path)
            .join(sp)
            .to_string_lossy()
            .to_string()
    } else {
        workdir.path
    };

    // Execute each prerun action, waiting for each to complete
    let mut count = 0;
    for action in prerun_actions {
        let listener = Arc::new(TauriExecutionListener::new(
            app_handle.clone(),
            branch_id.to_string(),
            action.id.clone(),
            action.name.clone(),
            Arc::clone(act_registry),
        ));

        let metadata = ActionMetadata {
            action_id: action.id.clone(),
            action_name: action.name.clone(),
            auto_commit: action.auto_commit,
        };

        // execute_and_wait runs the action and waits for it to finish,
        // regardless of success or failure (task requirement)
        match executor
            .execute_and_wait(action.command, working_dir.clone(), metadata, listener)
            .await
        {
            Ok(_execution_id) => {
                count += 1;
                log::info!(
                    "[project_mcp] prerun action '{}' completed for branch {}",
                    action.id,
                    branch_id
                );
            }
            Err(e) => {
                log::warn!(
                    "[project_mcp] prerun action '{}' failed (continuing): {e}",
                    action.id
                );
                count += 1; // count even if failed — we waited for it
            }
        }
    }

    Ok(count)
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
pub async fn start_project_mcp_server(
    project_id: String,
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
    action_executor: Option<Arc<ActionExecutor>>,
    action_registry: Option<Arc<ActionRegistry>>,
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
