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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub created_at: i64,
    pub updated_at: i64,
    /// PID of the Staged process that owns this session while it is running.
    /// Used on startup to detect sessions orphaned by a dead process.
    pub owner_pid: Option<u32>,
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
            created_at: now,
            updated_at: now,
            owner_pid: Some(std::process::id()),
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
            created_at: now,
            updated_at: now,
            owner_pid: None,
        }
    }

    pub fn with_provider(mut self, provider: &str) -> Self {
        self.provider = Some(provider.to_string());
        self
    }

    pub fn with_agent(mut self, agent_id: &str) -> Self {
        self.agent_id = Some(agent_id.to_string());
        self
    }
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
}

impl Note {
    pub fn new(branch_id: &str, title: &str, content: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            branch_id: branch_id.to_string(),
            session_id: None,
            title: title.to_string(),
            content: content.to_string(),
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
}

impl ProjectNote {
    pub fn new(project_id: &str, title: &str, content: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            session_id: None,
            title: title.to_string(),
            content: content.to_string(),
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
}
