//! MCP server for project sessions.
//! Exposes `start_repo_session` and `add_project_repo` tools to the agent.

use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::sse_server::SseServer;
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use tauri::AppHandle;

use crate::session_runner::{SessionConfig, SessionRegistry};
use crate::store::{Session, SessionStatus, Store};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct StartRepoSessionParams {
    /// GitHub repo slug present in the project, e.g. "org/repo".
    pub repo: String,
    /// Instructions to give the agent.
    pub instructions: String,
    /// Optional ACP provider ID (e.g. "claude", "goose").
    pub provider: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddProjectRepoParams {
    /// GitHub repo slug to add, e.g. "org/repo".
    pub github_repo: String,
    /// Optional branch name (defaults to project's inferred name).
    pub branch_name: Option<String>,
}

#[derive(Clone)]
struct ProjectToolsHandler {
    tool_router: ToolRouter<Self>,
    project_id: String,
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
}

impl ProjectToolsHandler {
    fn new(
        project_id: String,
        store: Arc<Store>,
        registry: Arc<SessionRegistry>,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            tool_router: Self::tool_router(),
            project_id,
            store,
            registry,
            app_handle,
        }
    }
}

#[tool_router]
impl ProjectToolsHandler {
    #[tool(
        description = "Start an agent session in one of the project's repositories. Waits for completion and returns the outcome. Use this to make changes or run tasks in a specific repo."
    )]
    async fn start_repo_session(
        &self,
        Parameters(p): Parameters<StartRepoSessionParams>,
    ) -> String {
        // Find the matching project repo
        let repos = match self.store.list_project_repos(&self.project_id) {
            Ok(r) => r,
            Err(e) => return format!("Error listing repos: {e}"),
        };
        let repo = match repos
            .iter()
            .find(|r| r.github_repo == p.repo || r.github_repo.ends_with(&format!("/{}", p.repo)))
        {
            Some(r) => r.clone(),
            None => {
                let available = repos
                    .iter()
                    .map(|r| r.github_repo.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return format!(
                    "Repository '{}' not found in project. Available repos: {}",
                    p.repo, available
                );
            }
        };

        // Find the branch for this repo to get workspace_name if remote
        let branches = match self.store.list_branches_for_project(&self.project_id) {
            Ok(b) => b,
            Err(e) => return format!("Error listing branches: {e}"),
        };
        let workspace_name = branches
            .iter()
            .find(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str()))
            .and_then(|b| b.workspace_name.clone());

        // Determine working directory
        let working_dir = crate::paths::repos_dir()
            .map(|d| d.join(&repo.github_repo))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

        // Create the session record
        let mut session = Session::new_running(&p.instructions, &working_dir);
        if let Some(ref prov) = p.provider {
            session = session.with_provider(prov);
        }
        if let Err(e) = self.store.create_session(&session) {
            return format!("Error creating session: {e}");
        }
        let session_id = session.id.clone();

        // Start the agent (returns immediately; work happens on background thread)
        let start_result = crate::session_runner::start_session(
            SessionConfig {
                session_id: session_id.clone(),
                prompt: p.instructions,
                working_dir,
                agent_session_id: None,
                pre_head_sha: None,
                provider: p.provider,
                workspace_name,
                extra_env: vec![],
                mcp_project_id: None,
            },
            Arc::clone(&self.store),
            self.app_handle.clone(),
            Arc::clone(&self.registry),
        );
        if let Err(e) = start_result {
            return format!("Error starting session: {e}");
        }

        // Poll until the session reaches a terminal state
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match self.store.get_session(&session_id) {
                Ok(Some(s)) if s.status != SessionStatus::Running => {
                    let outcome = match s.status {
                        SessionStatus::Completed => "completed",
                        SessionStatus::Cancelled => "cancelled",
                        _ => "failed",
                    };
                    return format!(r#"{{"session_id": "{session_id}", "outcome": "{outcome}"}}"#);
                }
                Ok(_) => continue,
                Err(e) => return format!("Error polling session status: {e}"),
            }
        }
    }

    #[tool(
        description = "Add a GitHub repository to the current project. Use this when the task requires a repository that isn't yet in the project."
    )]
    async fn add_project_repo(&self, Parameters(p): Parameters<AddProjectRepoParams>) -> String {
        match crate::project_commands::add_project_repo_impl(
            Arc::clone(&self.store),
            self.project_id.clone(),
            p.github_repo,
            p.branch_name,
            None,
            None,
        )
        .await
        {
            Ok(repo) => format!(
                r#"{{"repo_id": "{}", "message": "Added repository {} to project"}}"#,
                repo.id, repo.github_repo
            ),
            Err(e) => format!("Error adding repo: {e}"),
        }
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
pub async fn start_project_mcp_server(
    project_id: String,
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
) -> Result<(u16, JoinHandle<()>), String> {
    // Pre-bind a temp listener to discover a free port
    let temp = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind temp listener: {e}"))?;
    let port = temp
        .local_addr()
        .map_err(|e| format!("Failed to get local address: {e}"))?
        .port();
    drop(temp);

    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .map_err(|e| format!("Failed to parse address: {e}"))?;

    let handler = ProjectToolsHandler::new(project_id, store, registry, app_handle);

    let server = SseServer::serve(addr)
        .await
        .map_err(|e| format!("Failed to start MCP SSE server: {e}"))?;

    let ct = server.with_service(move || handler.clone());

    // Spawn a local task that keeps ct alive (dropped when LocalSet is dropped)
    let handle = tokio::task::spawn_local(async move {
        ct.cancelled().await;
    });

    Ok((port, handle))
}
