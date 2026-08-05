//! Domain types for Staged persistence.

use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::now_timestamp;

use crate::git::Span;

// =============================================================================
// Projects
// =============================================================================

/// Where a project's branches run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectLocation {
    /// Local git worktrees on this machine.
    Local,
    /// Remote Blox workstations.
    Remote,
}

impl ProjectLocation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

impl FromStr for ProjectLocation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(Self::Local),
            "remote" => Ok(Self::Remote),
            other => Err(format!("unknown project location: {other}")),
        }
    }
}

/// A tracked repository (user opt-in).
///
/// Projects are identified by their `id` and may share the same GitHub
/// `owner/repo` slug when they use different subpaths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    /// User-facing project name.
    pub name: String,
    /// Primary repository identifier, e.g. `"owner/repo"`.
    /// Optional so projects can be created before a repo is attached.
    pub github_repo: Option<String>,
    /// Where this project's branches run by default.
    pub location: ProjectLocation,
    pub subpath: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Project {
    /// Backwards-compatible constructor: creates a project named from a repo.
    pub fn new(github_repo: &str) -> Self {
        let fallback_name = github_repo
            .rsplit('/')
            .next()
            .unwrap_or(github_repo)
            .to_string();
        Self::named(&fallback_name).with_primary_repo(github_repo)
    }

    pub fn named(name: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            github_repo: None,
            location: ProjectLocation::Local,
            subpath: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_primary_repo(mut self, github_repo: &str) -> Self {
        self.github_repo = Some(github_repo.to_string());
        self
    }

    pub fn with_subpath(mut self, subpath: String) -> Self {
        if let Some(repo_name) = self.repo_name().map(str::to_string) {
            if self.name == repo_name {
                self.name = format!("{repo_name} ({subpath})");
            }
        }
        self.subpath = Some(subpath);
        self
    }

    /// Extract the repo name (last component of `owner/repo`) if set.
    pub fn repo_name(&self) -> Option<&str> {
        self.github_repo
            .as_deref()
            .map(|repo| repo.rsplit('/').next().unwrap_or(repo))
    }

    pub fn primary_repo(&self) -> Option<&str> {
        self.github_repo.as_deref()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRepo {
    pub id: String,
    pub project_id: String,
    pub github_repo: String,
    /// Preferred branch name for this repository inside the project.
    pub branch_name: String,
    pub subpath: Option<String>,
    pub is_primary: bool,
    pub reason: Option<String>,
    /// For fork PRs, the head (fork) repo that differs from `github_repo` (the
    /// base repo used for cloning and API calls).  `None` when the PR is not
    /// from a fork.
    pub head_repo: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ProjectRepo {
    pub fn new(
        project_id: &str,
        github_repo: &str,
        branch_name: &str,
        subpath: Option<String>,
    ) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            github_repo: github_repo.to_string(),
            branch_name: branch_name.to_string(),
            subpath,
            is_primary: false,
            reason: None,
            head_repo: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn primary(mut self) -> Self {
        self.is_primary = true;
        self
    }
}

// =============================================================================
// Branches
// =============================================================================

/// Whether a branch is backed by a local git worktree or a remote Blox workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BranchType {
    /// Local git worktree on this machine.
    Local,
    /// Remote Blox workspace.
    Remote,
}

impl BranchType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

impl FromStr for BranchType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(Self::Local),
            "remote" => Ok(Self::Remote),
            other => Err(format!("unknown branch type: {other}")),
        }
    }
}

/// Lifecycle status of a remote Blox workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceStatus {
    /// Workspace is being provisioned.
    Starting,
    /// Workspace is running and ready.
    Running,
    /// Workspace has been stopped (can be restarted).
    Stopped,
    /// Workspace has been suspended due to inactivity (can be resumed).
    Suspended,
    /// Workspace encountered an error.
    Error,
}

impl WorkspaceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Suspended => "suspended",
            Self::Error => "error",
        }
    }
}

impl FromStr for WorkspaceStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "stopped" => Ok(Self::Stopped),
            "suspended" => Ok(Self::Suspended),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown workspace status: {other}")),
        }
    }
}

/// A logical branch we manage. The branch's working directory (if any) is
/// tracked separately in the `workdirs` table — see `Workdir`.
///
/// Branches can be **local** (backed by a git worktree on this machine) or
/// **remote** (backed by a Blox workspace). Remote branches store additional
/// metadata: the workspace name and its lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub id: String,
    pub project_id: String,
    /// Repository inside the project this branch belongs to.
    pub project_repo_id: Option<String>,
    pub branch_name: String,
    pub base_branch: String,
    pub pr_number: Option<u64>,
    /// Whether this branch is local or remote. Defaults to `Local`.
    pub branch_type: BranchType,
    /// The Blox workspace name (remote branches only).
    pub workspace_name: Option<String>,
    /// Current lifecycle status of the workspace (remote branches only).
    pub workspace_status: Option<WorkspaceStatus>,
    /// PR state: "OPEN", "CLOSED", "MERGED", etc.
    pub pr_state: Option<String>,
    /// Combined checks status: "SUCCESS", "FAILURE", "PENDING", etc.
    pub pr_checks_status: Option<String>,
    /// Review decision: "APPROVED", "CHANGES_REQUESTED", "REVIEW_REQUIRED", etc.
    pub pr_review_decision: Option<String>,
    /// Whether the PR can be merged (not blocked by conflicts, required checks, etc.)
    pub pr_mergeable: Option<bool>,
    /// Whether the PR is a draft
    pub pr_draft: Option<bool>,
    /// GitHub URL to the PR
    pub pr_url: Option<String>,
    /// When the PR was last updated on GitHub (milliseconds since epoch)
    pub pr_updated_at: Option<i64>,
    /// When we last fetched PR status from GitHub (milliseconds since epoch)
    pub pr_fetched_at: Option<i64>,
    /// The SHA of the PR's head commit on GitHub
    pub pr_head_sha: Option<String>,
    /// Whether the branch has completed its initial setup (worktree created
    /// and prerun actions have had the opportunity to run).
    pub setup_complete: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Branch {
    /// Create a new local branch.
    pub fn new(project_id: &str, branch_name: &str, base_branch: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            project_repo_id: None,
            branch_name: branch_name.to_string(),
            base_branch: base_branch.to_string(),
            pr_number: None,
            branch_type: BranchType::Local,
            workspace_name: None,
            workspace_status: None,
            pr_state: None,
            pr_checks_status: None,
            pr_review_decision: None,
            pr_mergeable: None,
            pr_draft: None,
            pr_url: None,
            pr_updated_at: None,
            pr_fetched_at: None,
            pr_head_sha: None,
            setup_complete: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a new remote branch backed by a Blox workspace.
    pub fn new_remote(
        project_id: &str,
        branch_name: &str,
        base_branch: &str,
        workspace_name: &str,
    ) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            project_repo_id: None,
            branch_name: branch_name.to_string(),
            base_branch: base_branch.to_string(),
            pr_number: None,
            branch_type: BranchType::Remote,
            workspace_name: Some(workspace_name.to_string()),
            workspace_status: Some(WorkspaceStatus::Starting),
            pr_state: None,
            pr_checks_status: None,
            pr_review_decision: None,
            pr_mergeable: None,
            pr_draft: None,
            pr_url: None,
            pr_updated_at: None,
            pr_fetched_at: None,
            pr_head_sha: None,
            setup_complete: false,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_pr(mut self, pr_number: u64) -> Self {
        self.pr_number = Some(pr_number);
        self
    }

    pub fn with_project_repo(mut self, project_repo_id: &str) -> Self {
        self.project_repo_id = Some(project_repo_id.to_string());
        self
    }
}

// =============================================================================
// Workdirs
// =============================================================================

/// A filesystem location where git operations can happen.
///
/// A workdir is a pooled resource owned by a project. It may be a
/// `git worktree` or the main repository checkout itself. A branch is
/// assigned to a workdir when it needs one (agent work, user browsing)
/// and released when it doesn't.
///
/// - `branch_id = Some(...)` → the workdir is **occupied** by that branch.
/// - `branch_id = None` → the workdir is **available** for assignment.
///
/// In "full" mode (current default), each branch gets its own dedicated
/// workdir created via `git worktree add`. In future "shared" or "pool"
/// modes, multiple branches will share a smaller set of workdirs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workdir {
    pub id: String,
    pub project_id: String,
    pub path: String,
    pub branch_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Workdir {
    pub fn new(project_id: &str, path: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            path: path.to_string(),
            branch_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_branch(mut self, branch_id: &str) -> Self {
        self.branch_id = Some(branch_id.to_string());
        self
    }
}

// =============================================================================
// Sessions
// =============================================================================

/// Session status — the single source of truth for session lifecycle.
///
/// Used both for persisted state in SQLite and for in-memory tracking by
/// the SessionManager. `Running` means the agent connection is alive
/// (whether idle or actively streaming). The SessionManager tracks
/// `is_processing` separately as a transient in-memory flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Queued,
    Running,
    Completed,
    Error,
    Cancelled,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "error" | "failed" => Some(Self::Error),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Why a session reached its terminal state.
///
/// Stored alongside `SessionStatus` to distinguish between different kinds
/// of completion, cancellation, and failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionReason {
    /// Agent finished its turn normally (`prompt()` → `Ok`).
    TurnComplete,
    /// Direct user stop, legacy unknown stop, or generic cancellation.
    Interrupted,
    /// A parent project session stopped this repo session.
    ProjectSessionInterrupted,
    /// Agent process exited or connection was lost.
    Crashed,
    /// Staged closed while the session was still running.
    AppQuit,
    /// Legacy sessions or indeterminate cause.
    Unknown,
}

impl CompletionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TurnComplete => "turn_complete",
            Self::Interrupted => "interrupted",
            Self::ProjectSessionInterrupted => "project_session_interrupted",
            Self::Crashed => "crashed",
            Self::AppQuit => "app_quit",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "turn_complete" => Some(Self::TurnComplete),
            "interrupted" => Some(Self::Interrupted),
            "project_session_interrupted" => Some(Self::ProjectSessionInterrupted),
            "crashed" => Some(Self::Crashed),
            "app_quit" => Some(Self::AppQuit),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// A unit of AI work. Sessions are standalone records; artifacts (commits,
/// notes, reviews) point at them via `session_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub prompt: String,
    pub status: SessionStatus,
    pub working_dir: String,
    /// Which agent provider ran this session (e.g. "goose", "claude").
    /// Protocol-agnostic — survives a switch from ACP to another protocol.
    pub provider: Option<String>,
    /// Protocol-level session ID used by the agent for conversation
    /// resumption (e.g. the ACP session ID returned by `new_session`).
    pub agent_id: Option<String>,
    pub error_message: Option<String>,
    /// Why the session reached its terminal state. `None` while running/queued.
    pub completion_reason: Option<CompletionReason>,
    pub created_at: i64,
    pub updated_at: i64,
    /// PID of the Staged process that owns this session while it is running.
    /// Used on startup to detect sessions orphaned by a dead process.
    pub owner_pid: Option<u32>,
    /// Pipeline execution state. When present, the session was started via a
    /// command pipeline (deterministic steps before/instead of AI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<PipelineExecution>,
    /// Selected ACP config values to apply before prompting the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_config_selection: Option<AcpConfigSelection>,
    /// Latest session title pushed by the agent via ACP `session_info_update`.
    /// Used as an interim display name while the session is running.
    #[serde(default)]
    pub acp_title: Option<String>,
    /// Branch this session belongs to, for sessions that create no artifact.
    ///
    /// Branch-scoped sessions are normally found through their commit, note, or
    /// review row. Push pipelines have none of those, so they record the branch
    /// here to stay visible to the branch queue. `None` for artifact-backed
    /// sessions and for project-level sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
}

/// Persistent follow-up message waiting to be sent to an existing session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedSessionMessage {
    pub id: String,
    pub session_id: String,
    pub branch_id: Option<String>,
    pub content: String,
    pub image_ids: Vec<String>,
    pub status: QueuedSessionMessageStatus,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub claimed_at: Option<i64>,
    pub owner_pid: Option<u32>,
    pub sent_message_id: Option<i64>,
}

impl QueuedSessionMessage {
    pub fn new(
        session_id: &str,
        branch_id: Option<&str>,
        content: &str,
        image_ids: &[String],
    ) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            branch_id: branch_id.map(str::to_string),
            content: content.to_string(),
            image_ids: image_ids.to_vec(),
            status: QueuedSessionMessageStatus::Queued,
            last_error: None,
            created_at: now,
            updated_at: now,
            claimed_at: None,
            owner_pid: None,
            sent_message_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueuedSessionMessageStatus {
    Queued,
    Sending,
    Sent,
}

impl QueuedSessionMessageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sending => "sending",
            Self::Sent => "sent",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "sending" => Some(Self::Sending),
            "sent" => Some(Self::Sent),
            _ => None,
        }
    }
}

impl Session {
    pub fn new_running(prompt: &str, working_dir: &Path) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            prompt: prompt.to_string(),
            status: SessionStatus::Running,
            working_dir: working_dir.to_string_lossy().to_string(),
            provider: None,
            agent_id: None,
            error_message: None,
            completion_reason: None,
            created_at: now,
            updated_at: now,
            owner_pid: Some(std::process::id()),
            pipeline: None,
            acp_config_selection: None,
            acp_title: None,
            branch_id: None,
        }
    }

    /// Create a queued session. The prompt is stored but no agent is spawned.
    /// The working_dir is left empty since it will be resolved when the session
    /// is actually started (drained).
    pub fn new_queued(prompt: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            prompt: prompt.to_string(),
            status: SessionStatus::Queued,
            working_dir: String::new(),
            provider: None,
            agent_id: None,
            error_message: None,
            completion_reason: None,
            created_at: now,
            updated_at: now,
            owner_pid: None,
            pipeline: None,
            acp_config_selection: None,
            acp_title: None,
            branch_id: None,
        }
    }

    pub fn with_provider(mut self, provider: &str) -> Self {
        self.provider = Some(provider.to_string());
        self
    }

    /// Link an artifact-less session to its branch so the branch queue sees it.
    pub fn with_branch(mut self, branch_id: &str) -> Self {
        self.branch_id = Some(branch_id.to_string());
        self
    }

    pub fn with_agent(mut self, agent_id: &str) -> Self {
        self.agent_id = Some(agent_id.to_string());
        self
    }

    pub fn with_acp_config_selection(mut self, selection: AcpConfigSelection) -> Self {
        self.acp_config_selection = Some(selection);
        self
    }
}

/// Session-level ACP config selections keyed by product-facing category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<AcpConfigValueSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<AcpConfigValueSelection>,
}

/// Selected value for one ACP `session/set_config_option` config ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigValueSelection {
    pub config_id: String,
    pub value_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

// =============================================================================
// Commits
// =============================================================================

/// Our metadata about a commit on a branch.
///
/// Git is authoritative for all commit data (message, diff, tree, etc.).
/// This record exists to link commits to the AI session that produced them
/// and to represent pending commits (where `sha` is `None` until the
/// commit lands in git).
///
/// If a commit disappears from git (rebase, reset), this row is harmlessly
/// orphaned — the frontend should cross-reference with actual git history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub id: String,
    pub branch_id: String,
    pub sha: Option<String>,
    pub session_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Commit {
    /// Create a pending commit (session in progress, SHA not yet known).
    pub fn new_pending(branch_id: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            branch_id: branch_id.to_string(),
            sha: None,
            session_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a completed commit with a known SHA.
    pub fn new_with_sha(branch_id: &str, sha: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            branch_id: branch_id.to_string(),
            sha: Some(sha.to_string()),
            session_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }
}

// =============================================================================
// Session Messages
// =============================================================================

/// Role for session messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    ToolCall,
    ToolResult,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool_call" => Some(Self::ToolCall),
            "tool_result" => Some(Self::ToolResult),
            _ => None,
        }
    }
}

/// A message in a session transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub id: i64,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: i64,
    /// Image IDs attached to this message (user messages only).
    /// Stored as a JSON array string in the DB, deserialized to a Vec here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_ids: Vec<String>,
    #[serde(flatten)]
    pub acp: AcpMessageMetadata,
}

/// ACP metadata attached to a transcript row.
///
/// These fields preserve richer ACP v1 events without changing the legacy
/// transcript projection consumed by the current UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AcpMessageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_event_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_protocol_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_agent_capabilities: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_auth_methods: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_agent_info: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_tool_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_tool_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_raw_input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_raw_output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_content: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_locations: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_usage: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_info: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_config_options: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_session_mode_state: Option<serde_json::Value>,
}

// =============================================================================
// Notes
// =============================================================================

/// An AI-generated markdown document tied to a branch.
///
/// Notes may be created empty (with a `session_id`) while the AI is still
/// generating content. The frontend can check the linked session's status
/// to determine if the note is still in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub branch_id: String,
    pub session_id: Option<String>,
    pub title: String,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// When the AI session finished producing this note's content.
    /// `None` while the session is still running.
    pub completed_at: Option<i64>,
    /// AI-suggested prompt for a follow-up commit session.
    pub suggested_next_commit_step: Option<String>,
    /// AI-suggested prompt for a follow-up note session.
    pub suggested_next_note_step: Option<String>,
}

impl Note {
    pub fn new(branch_id: &str, title: &str, content: &str) -> Self {
        let now = now_timestamp();
        let has_content = !content.is_empty();
        Self {
            id: Uuid::new_v4().to_string(),
            branch_id: branch_id.to_string(),
            session_id: None,
            title: title.to_string(),
            content: content.to_string(),
            created_at: now,
            updated_at: now,
            completed_at: if has_content { Some(now) } else { None },
            suggested_next_commit_step: None,
            suggested_next_note_step: None,
        }
    }

    pub fn with_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }
}

// =============================================================================
// Project Notes
// =============================================================================

/// A note scoped to a project (not a specific branch).
///
/// Project notes capture cross-cutting context, research, or decisions
/// that apply to the project as a whole. They are injected into every
/// branch session's context so the agent has project-level awareness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectNote {
    pub id: String,
    pub project_id: String,
    pub session_id: Option<String>,
    pub title: String,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// When the AI session finished producing this project note's content.
    /// `None` while the session is still running.
    pub completed_at: Option<i64>,
    /// AI-suggested prompt for a follow-up commit session.
    pub suggested_next_commit_step: Option<String>,
    /// AI-suggested prompt for a follow-up note session.
    pub suggested_next_note_step: Option<String>,
    /// Resolved session status (e.g. "running", "completed", "cancelled").
    /// Populated at query time via `resolve_session_status()`.
    #[serde(skip_deserializing)]
    pub session_status: Option<String>,
    /// Why the session reached its terminal state.
    #[serde(skip_deserializing)]
    pub completion_reason: Option<String>,
}

impl ProjectNote {
    pub fn new(project_id: &str, title: &str, content: &str) -> Self {
        let now = now_timestamp();
        let has_content = !content.is_empty();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            session_id: None,
            title: title.to_string(),
            content: content.to_string(),
            created_at: now,
            updated_at: now,
            completed_at: if has_content { Some(now) } else { None },
            suggested_next_commit_step: None,
            suggested_next_note_step: None,
            session_status: None,
            completion_reason: None,
        }
    }

    pub fn with_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }
}

// =============================================================================
// Images
// =============================================================================

/// An image attached to a branch.
///
/// The image file is stored on disk at
/// `<project_worktree_root>/images/<id>.<ext>`. The `project_id` field
/// determines the filesystem location; the `filename` field preserves the
/// original upload name (and its extension).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub id: String,
    pub branch_id: Option<String>,
    pub project_id: String,
    pub session_id: Option<String>,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub created_at: i64,
}

/// Sentinel value stored in `images.session_id` for images that are being
/// composed in a modal but haven't been submitted yet.  The branch-timeline
/// query (`WHERE session_id IS NULL`) naturally excludes these, so they never
/// appear in the timeline.  When a session is actually started the runner
/// overwrites this with the real session ID via `set_images_session_id`.
/// On app startup any images still marked pending are cleaned up.
pub const PENDING_SESSION_ID: &str = "pending";

impl Image {
    /// Create a new image record.
    ///
    /// When `pending` is true the image is created with
    /// `session_id = "pending"` so it is invisible in the branch timeline
    /// until a session is started (at which point the runner overwrites it
    /// with the real session ID).  Pass `false` for images that should
    /// appear in the timeline immediately (e.g. direct branch-card drops).
    pub fn new(
        branch_id: Option<&str>,
        project_id: &str,
        filename: &str,
        mime_type: &str,
        size_bytes: i64,
        pending: bool,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            branch_id: branch_id.map(|s| s.to_string()),
            project_id: project_id.to_string(),
            session_id: if pending {
                Some(PENDING_SESSION_ID.to_string())
            } else {
                None
            },
            filename: filename.to_string(),
            mime_type: mime_type.to_string(),
            size_bytes,
            created_at: now_timestamp(),
        }
    }

    pub fn with_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }
}

// =============================================================================
// Recent Repos
// =============================================================================

/// A recently used repository to simplify project creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRepo {
    pub id: String,
    pub github_repo: String,
    pub subpath: Option<String>,
    pub last_used_at: i64,
}

impl RecentRepo {
    pub fn new(github_repo: &str, subpath: Option<String>) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            github_repo: github_repo.to_string(),
            subpath,
            last_used_at: now,
        }
    }
}

// =============================================================================
// Project Actions
// =============================================================================

/// Re-export ActionType and RunDetectionMode from builderbot-actions crate as the single source of truth.
pub use builderbot_actions::ActionType;
pub use builderbot_actions::RunDetectionMode;

/// Durable action context keyed by GitHub repo + optional subpath.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionContext {
    pub id: String,
    pub github_repo: String,
    pub subpath: Option<String>,
    pub has_detected_actions: bool,
    pub detecting_actions: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ActionContext {
    pub fn new(github_repo: impl Into<String>, subpath: Option<String>) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            github_repo: github_repo.into(),
            subpath,
            has_detected_actions: false,
            detecting_actions: false,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A configurable action (build, test, format, etc.) scoped to a repo context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoAction {
    pub id: String,
    pub context_id: String,
    pub name: String,
    pub command: String,
    pub action_type: ActionType,
    pub sort_order: i32,
    pub auto_commit: bool,
    pub run_detection_mode: Option<RunDetectionMode>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl RepoAction {
    pub fn new(
        context_id: String,
        name: String,
        command: String,
        action_type: ActionType,
        sort_order: i32,
    ) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            context_id,
            name,
            command,
            action_type,
            sort_order,
            auto_commit: false,
            run_detection_mode: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_auto_commit(mut self, auto_commit: bool) -> Self {
        self.auto_commit = auto_commit;
        self
    }
}

// =============================================================================
// Reviews
// =============================================================================

/// What scope of changes a review covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewScope {
    /// Review of a single commit's diff (parent..commit).
    Commit,
    /// Review of the full branch diff (base..commit) as of a point in time.
    Branch,
}

impl ReviewScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Branch => "branch",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "commit" => Some(Self::Commit),
            "branch" => Some(Self::Branch),
            _ => None,
        }
    }
}

/// A review anchored to a specific commit on a branch.
///
/// The diff range is derived, not stored:
/// - **Commit scope**: `commit_sha~1..commit_sha`
/// - **Branch scope**: `base_branch..commit_sha` (branch knows its base)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub id: String,
    pub branch_id: String,
    pub commit_sha: String,
    pub scope: ReviewScope,
    pub session_id: Option<String>,
    /// AI-generated one-sentence title summarising the review's confidence.
    pub title: Option<String>,
    /// Whether this review was automatically generated (not user-initiated).
    pub is_auto: bool,
    /// Paths that have been marked as reviewed.
    pub reviewed: Vec<String>,
    /// Comments attached to specific locations.
    pub comments: Vec<Comment>,
    /// Paths of reference files (files outside the diff that were viewed).
    pub reference_files: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// When the AI session finished producing this review.
    /// `None` while the session is still running.
    pub completed_at: Option<i64>,
    /// The AI provider used by the session that created this review.
    /// Only populated by `find_fresh_auto_review`; `None` elsewhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_provider: Option<String>,
}

impl Review {
    pub fn new(branch_id: &str, commit_sha: &str, scope: ReviewScope) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            branch_id: branch_id.to_string(),
            commit_sha: commit_sha.to_string(),
            scope,
            session_id: None,
            title: None,
            is_auto: false,
            reviewed: Vec::new(),
            comments: Vec::new(),
            reference_files: Vec::new(),
            created_at: now,
            updated_at: now,
            completed_at: None,
            session_provider: None,
        }
    }

    pub fn with_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    pub fn with_auto(mut self) -> Self {
        self.is_auto = true;
        self
    }
}

/// Who authored a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentAuthor {
    /// Comment written by the user.
    User,
    /// Comment generated by an AI agent.
    Agent,
}

impl CommentAuthor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// The type/severity of a review comment.
///
/// AI-generated comments include a type so the frontend can decide how to
/// display them: `information` comments become hold-A annotations while
/// other types render as normal inline comments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentType {
    /// Informational note — rendered as an annotation overlay.
    Information,
    /// Suggestion for improvement.
    Suggestion,
    /// Warning about a potential issue.
    Warning,
    /// Bug or correctness issue.
    Issue,
}

impl CommentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Information => "information",
            Self::Suggestion => "suggestion",
            Self::Warning => "warning",
            Self::Issue => "issue",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "information" => Some(Self::Information),
            "suggestion" => Some(Self::Suggestion),
            "warning" => Some(Self::Warning),
            "issue" => Some(Self::Issue),
            _ => None,
        }
    }
}

/// A comment attached to a specific location in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub path: String,
    pub span: Span,
    pub content: String,
    pub author: CommentAuthor,
    /// The type/severity of this comment. `None` for user-authored comments.
    pub comment_type: Option<CommentType>,
    pub created_at: i64,
    /// When the comment was soft-deleted. `None` means active.
    pub deleted_at: Option<i64>,
    /// The GitHub API comment ID, set after posting to GitHub.
    pub github_comment_id: Option<i64>,
    /// The type of GitHub comment: "review" (inline) or "issue" (fallback).
    pub github_comment_type: Option<String>,
    /// Whether the local content has been edited since the last GitHub sync.
    pub github_comment_stale: bool,
    /// The note session started from this comment's "Note" button, if any.
    pub note_session_id: Option<String>,
    /// The commit session started from this comment's "Commit" button, if any.
    pub commit_session_id: Option<String>,
}

impl Comment {
    pub fn new(path: impl Into<String>, span: Span, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            path: path.into(),
            span,
            content: content.into(),
            author: CommentAuthor::User,
            comment_type: None,
            created_at: now_timestamp(),
            deleted_at: None,
            github_comment_id: None,
            github_comment_type: None,
            github_comment_stale: false,
            note_session_id: None,
            commit_session_id: None,
        }
    }

    pub fn with_author(mut self, author: CommentAuthor) -> Self {
        self.author = author;
        self
    }

    pub fn with_comment_type(mut self, comment_type: CommentType) -> Self {
        self.comment_type = Some(comment_type);
        self
    }
}

// =============================================================================
// Pipelines
// =============================================================================

/// A step in a command pipeline — either a deterministic shell command or an
/// AI handoff. Pipelines run before (and sometimes instead of) an AI session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PipelineStep {
    /// Run a shell command deterministically.
    #[serde(rename_all = "camelCase")]
    Command {
        /// Human-readable label shown in UI (e.g. "Push to remote").
        label: String,
        /// The shell command to execute.
        command: String,
        /// What to do when this command fails.
        on_failure: FailureStrategy,
    },
    /// Hand off to an AI session with context from prior steps.
    #[serde(rename_all = "camelCase")]
    AiHandoff {
        /// Human-readable label (e.g. "Write PR title and body").
        label: String,
        /// Prompt template — can reference `{step_outputs}` to inject
        /// stdout/stderr from prior command steps.
        prompt_template: String,
    },
}

/// What to do when a command step fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FailureStrategy {
    /// Stop the pipeline immediately, mark it as failed.
    /// If `marker` is set, the pipeline only aborts when the output contains
    /// that string (otherwise falls through to AI handoff).
    #[serde(rename_all = "camelCase")]
    Abort { marker: Option<String> },
    /// Hand off to AI to diagnose and fix the failure.
    #[serde(rename_all = "camelCase")]
    HandoffToAi { prompt_template: String },
    /// Skip this step and continue to the next.
    Continue,
}

/// Status of an individual pipeline step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

/// Whether a step is a deterministic command or an AI handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    Command,
    #[serde(alias = "aihandoff", alias = "aiHandoff")]
    AiHandoff,
}

/// Execution status of a single pipeline step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStepStatus {
    pub label: String,
    pub step_type: StepType,
    pub status: StepStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

/// Persisted alongside the session. Tracks the execution state of each step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineExecution {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PipelineKind>,
    /// Remote ref the rebase variant should target (without the `origin/` prefix).
    ///
    /// `None` means the pipeline targets the branch's configured base (today's
    /// default). `Some("feature-x")` records that a "Rebase onto Origin" was
    /// requested so the queued path can re-derive the same steps on dequeue
    /// rather than silently downgrading to a base rebase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebase_target: Option<String>,
    /// Whether the push variant force-pushes (`--force-with-lease`).
    ///
    /// Recorded so a queued push re-derives the same command on dequeue, and so
    /// the queue can tell a pending push from a pending force push. Always
    /// `false` for non-push pipelines.
    #[serde(default, skip_serializing_if = "is_false")]
    pub push_force: bool,
    pub steps: Vec<PipelineStepStatus>,
    pub current_step: usize,
    /// Set when pipeline completes without needing AI.
    pub completed_without_ai: bool,
}

impl PipelineExecution {
    /// Create a new pipeline execution from a list of step definitions.
    pub fn from_steps(steps: &[PipelineStep]) -> Self {
        let step_statuses = steps
            .iter()
            .map(|step| match step {
                PipelineStep::Command { label, .. } => PipelineStepStatus {
                    label: label.clone(),
                    step_type: StepType::Command,
                    status: StepStatus::Pending,
                    output: None,
                    error: None,
                    started_at: None,
                    completed_at: None,
                },
                PipelineStep::AiHandoff { label, .. } => PipelineStepStatus {
                    label: label.clone(),
                    step_type: StepType::AiHandoff,
                    status: StepStatus::Pending,
                    output: None,
                    error: None,
                    started_at: None,
                    completed_at: None,
                },
            })
            .collect();

        Self {
            kind: None,
            rebase_target: None,
            push_force: false,
            steps: step_statuses,
            current_step: 0,
            completed_without_ai: false,
        }
    }

    pub fn with_kind(mut self, kind: PipelineKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_rebase_target(mut self, target: String) -> Self {
        self.rebase_target = Some(target);
        self
    }

    pub fn with_push_force(mut self, force: bool) -> Self {
        self.push_force = force;
        self
    }
}

/// `skip_serializing_if` predicate so non-push pipelines persist no push flag.
fn is_false(value: &bool) -> bool {
    !*value
}

/// Durable identity for command pipelines that the branch queue schedules.
///
/// `Rebase` and `Squash` produce a commit and are linked to their branch through
/// a pending-commit artifact. `Push` and `Pull` produce no artifact and are
/// linked through `Session::branch_id` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipelineKind {
    Rebase,
    Squash,
    Push,
    Pull,
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;

    #[test]
    fn step_type_serializes_ai_handoff_as_snake_case() {
        let serialized = serde_json::to_string(&StepType::AiHandoff).unwrap();
        assert_eq!(serialized, "\"ai_handoff\"");
        assert_eq!(
            serde_json::from_str::<StepType>("\"ai_handoff\"").unwrap(),
            StepType::AiHandoff
        );
        assert_eq!(
            serde_json::from_str::<StepType>("\"aihandoff\"").unwrap(),
            StepType::AiHandoff
        );
    }

    #[test]
    fn pipeline_kind_is_optional_for_legacy_pipeline_json() {
        let execution: PipelineExecution =
            serde_json::from_str(r#"{"steps":[],"currentStep":0,"completedWithoutAi":false}"#)
                .unwrap();

        assert_eq!(execution.kind, None);
    }

    #[test]
    fn pipeline_rebase_target_is_optional_for_legacy_pipeline_json() {
        let execution: PipelineExecution = serde_json::from_str(
            r#"{"kind":"rebase","steps":[],"currentStep":0,"completedWithoutAi":false}"#,
        )
        .unwrap();

        assert_eq!(execution.rebase_target, None);
    }

    #[test]
    fn pipeline_push_force_is_optional_for_legacy_pipeline_json() {
        let execution: PipelineExecution =
            serde_json::from_str(r#"{"steps":[],"currentStep":0,"completedWithoutAi":false}"#)
                .unwrap();

        assert!(!execution.push_force);
    }

    #[test]
    fn pipeline_push_force_round_trips_and_stays_out_of_non_push_json() {
        let execution = PipelineExecution::from_steps(&[])
            .with_kind(PipelineKind::Push)
            .with_push_force(true);
        let json = serde_json::to_string(&execution).unwrap();

        assert!(json.contains("\"kind\":\"push\""));
        assert!(json.contains("\"pushForce\":true"));
        assert!(
            serde_json::from_str::<PipelineExecution>(&json)
                .unwrap()
                .push_force
        );

        let rebase = PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase);
        assert!(!serde_json::to_string(&rebase)
            .unwrap()
            .contains("pushForce"));
    }

    #[test]
    fn pipeline_rebase_target_round_trips() {
        let execution: PipelineExecution = serde_json::from_str(
            r#"{"kind":"rebase","rebaseTarget":"feature-x","steps":[],"currentStep":0,"completedWithoutAi":false}"#,
        )
        .unwrap();

        assert_eq!(execution.rebase_target.as_deref(), Some("feature-x"));
        let json = serde_json::to_string(&execution).unwrap();
        assert!(json.contains("\"rebaseTarget\":\"feature-x\""));
    }
}

// =============================================================================
// Suggested Repos (Repo Affinities)
// =============================================================================

/// A repo that has historically been used alongside the current project's repos.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedRepo {
    pub github_repo: String,
    pub subpath: Option<String>,
    pub score: i64,
}

// =============================================================================
// Repo Badges
// =============================================================================

/// A persistent short-name + color badge for a repository (optionally scoped by subpath).
///
/// Badges are shared across all projects — the same repo+subpath always gets the
/// same badge regardless of which project references it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoBadge {
    pub github_repo: String,
    pub subpath: String,
    pub short_name: String,
    pub hue: f64,
    pub created_at: i64,
    pub pinned: bool,
    pub pin_sort_order: Option<i32>,
    pub default_branch: Option<String>,
}
