use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Sentinel value representing the working tree (uncommitted changes).
pub const WORKDIR: &str = "WORKDIR";

/// A reference to a point in git history (or working tree)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum GitRef {
    /// The working tree (uncommitted changes)
    WorkingTree,
    /// The staging area / index (what has been `git add`'d)
    Index,
    /// Anything that resolves to a commit: SHA, branch, tag, origin/main, HEAD~3, etc.
    Rev(String),
    /// Merge-base between the default branch and HEAD.
    /// Resolved dynamically at diff-time to handle branch switches.
    MergeBase,
    /// Merge-base between a specific branch and a head ref.
    /// Format: [base_branch, head_ref] - computes merge-base(base_branch, head_ref)
    MergeBaseOf([String; 2]),
}

impl GitRef {
    /// String representation for git commands
    /// WorkingTree is represented as empty string (git uses working tree by default)
    /// MergeBase/MergeBaseOf should be resolved before calling this
    pub fn as_git_arg(&self) -> Option<&str> {
        match self {
            GitRef::WorkingTree => None,
            GitRef::Index => None,
            GitRef::Rev(s) => Some(s),
            GitRef::MergeBase => panic!("MergeBase must be resolved before use"),
            GitRef::MergeBaseOf(_) => panic!("MergeBaseOf must be resolved before use"),
        }
    }

    /// Display representation (@ for working tree, merge-base for MergeBase)
    pub fn display(&self) -> &str {
        match self {
            GitRef::WorkingTree => "@",
            GitRef::Index => "index",
            GitRef::Rev(s) => s,
            GitRef::MergeBase => "merge-base",
            GitRef::MergeBaseOf(_) => "merge-base",
        }
    }
}

/// What we're diffing - always base..head
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSpec {
    pub base: GitRef,
    pub head: GitRef,
}

impl DiffSpec {
    /// Uncommitted changes: HEAD..@
    pub fn uncommitted() -> Self {
        Self {
            base: GitRef::Rev("HEAD".to_string()),
            head: GitRef::WorkingTree,
        }
    }

    /// Last commit: HEAD~1..HEAD
    pub fn last_commit() -> Self {
        Self {
            base: GitRef::Rev("HEAD~1".to_string()),
            head: GitRef::Rev("HEAD".to_string()),
        }
    }

    /// Custom range
    pub fn custom(base: GitRef, head: GitRef) -> Self {
        Self { base, head }
    }

    /// Display as "base..head"
    pub fn display(&self) -> String {
        format!("{}..{}", self.base.display(), self.head.display())
    }
}

/// A contiguous range of lines (0-indexed, exclusive end)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Maximum file size (in bytes) for image preview. Files larger than this
/// fall back to the generic binary notice to avoid sending huge payloads.
pub const IMAGE_PREVIEW_MAX_BYTES: usize = 5 * 1024 * 1024; // 5 MB

/// Image extensions eligible for inline base64 preview.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Content of a file - either text lines, binary marker, or base64-encoded image
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FileContent {
    Text {
        lines: Vec<String>,
    },
    Binary,
    ImageBase64 {
        #[serde(rename = "mimeType")]
        mime_type: String,
        data: String,
    },
}

/// A file with its path and content
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub content: FileContent,
}

/// Summary of a file in the diff (for sidebar)
/// Status inferred: Added (before=None), Deleted (after=None),
/// Renamed (both Some, different paths), Modified (both Some, same path)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffSummary {
    pub before: Option<PathBuf>,
    pub after: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_lines: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_lines: Option<u32>,
}

impl FileDiffSummary {
    pub fn new(before: Option<PathBuf>, after: Option<PathBuf>) -> Self {
        Self {
            before,
            after,
            added_lines: None,
            deleted_lines: None,
        }
    }

    /// The primary path to use for this file (after if exists, else before)
    pub fn path(&self) -> &PathBuf {
        self.after.as_ref().or(self.before.as_ref()).unwrap()
    }

    pub fn is_added(&self) -> bool {
        self.before.is_none() && self.after.is_some()
    }

    pub fn is_deleted(&self) -> bool {
        self.before.is_some() && self.after.is_none()
    }

    pub fn is_renamed(&self) -> bool {
        match (&self.before, &self.after) {
            (Some(b), Some(a)) => b != a,
            _ => false,
        }
    }
}

/// Maps a region in before to a region in after
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alignment {
    pub before: Span,
    pub after: Span,
    /// True if this region contains changes
    pub changed: bool,
}

/// Full diff content for rendering a single file
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    /// File before the change (None if added)
    pub before: Option<File>,
    /// File after the change (None if deleted)
    pub after: Option<File>,
    /// How lines map between before/after
    pub alignments: Vec<Alignment>,
}
