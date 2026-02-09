//! Domain types for Staged persistence.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::now_timestamp;

use crate::git::Span;

// =============================================================================
// Projects
// =============================================================================

/// A tracked repository (user opt-in).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub repo_path: String,
    pub subpath: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Project {
    pub fn new(repo_path: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            repo_path: repo_path.to_string(),
            subpath: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_subpath(mut self, subpath: String) -> Self {
        self.subpath = Some(subpath);
        self
    }
}

// =============================================================================
// Branches
// =============================================================================

/// A logical branch we manage. The branch's working directory (if any) is
/// tracked separately in the `workdirs` table — see `Workdir`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub id: String,
    pub project_id: String,
    pub branch_name: String,
    pub base_branch: String,
    pub pr_number: Option<u64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Branch {
    pub fn new(project_id: &str, branch_name: &str, base_branch: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            branch_name: branch_name.to_string(),
            base_branch: base_branch.to_string(),
            pr_number: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_pr(mut self, pr_number: u64) -> Self {
        self.pr_number = Some(pr_number);
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
    Running,
    Completed,
    Error,
    Cancelled,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
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
    pub agent_id: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Session {
    pub fn new_running(prompt: &str) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            prompt: prompt.to_string(),
            status: SessionStatus::Running,
            agent_id: None,
            error_message: None,
            created_at: now,
            updated_at: now,
        }
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
// Project Actions
// =============================================================================

/// Action types for project actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    Build,
    Test,
    Format,
    Lint,
    Typecheck,
    Prerun,
    Custom,
}

impl ActionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Typecheck => "typecheck",
            Self::Prerun => "prerun",
            Self::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "build" => Some(Self::Build),
            "test" => Some(Self::Test),
            "format" => Some(Self::Format),
            "lint" => Some(Self::Lint),
            "typecheck" => Some(Self::Typecheck),
            "prerun" => Some(Self::Prerun),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// A configurable project action (build, test, format, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAction {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub command: String,
    pub action_type: ActionType,
    pub sort_order: i32,
    pub auto_commit: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ProjectAction {
    pub fn new(
        project_id: String,
        name: String,
        command: String,
        action_type: ActionType,
        sort_order: i32,
    ) -> Self {
        let now = now_timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            project_id,
            name,
            command,
            action_type,
            sort_order,
            auto_commit: false,
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
}

/// A comment attached to a specific location in a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub path: String,
    pub span: Span,
    pub content: String,
    pub created_at: i64,
}

impl Comment {
    pub fn new(path: impl Into<String>, span: Span, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            path: path.into(),
            span,
            content: content.into(),
            created_at: now_timestamp(),
        }
    }
}
