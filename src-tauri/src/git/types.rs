use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Sentinel value representing the working tree (uncommitted changes).
/// Used for DiffId storage keys.
pub const WORKDIR: &str = "WORKDIR";

/// Identifies a diff between two repository states for storage (reviews).
/// Uses resolved SHAs or WORKDIR sentinel, not symbolic refs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiffId {
    pub before: String,
    pub after: String,
}

impl DiffId {
    pub fn new(before: impl Into<String>, after: impl Into<String>) -> Self {
        Self {
            before: before.into(),
            after: after.into(),
        }
    }

    /// Returns true if this diff includes the working tree.
    pub fn is_working_tree(&self) -> bool {
        self.after == WORKDIR
    }
}

/// A reference to a point in git history (or working tree)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum GitRef {
    /// The working tree (uncommitted changes)
    WorkingTree,
    /// Anything that resolves to a commit: SHA, branch, tag, origin/main, HEAD~3, etc.
    Rev(String),
    /// Merge-base between the default branch and HEAD.
    /// Resolved dynamically at diff-time to handle branch switches.
    MergeBase,
}

impl GitRef {
    /// String representation for git commands
    /// WorkingTree is represented as empty string (git uses working tree by default)
    /// MergeBase should be resolved before calling this
    pub fn as_git_arg(&self) -> Option<&str> {
        match self {
            GitRef::WorkingTree => None,
            GitRef::Rev(s) => Some(s),
            GitRef::MergeBase => panic!("MergeBase must be resolved before use"),
        }
    }

    /// Display representation (@ for working tree, merge-base for MergeBase)
    pub fn display(&self) -> &str {
        match self {
            GitRef::WorkingTree => "@",
            GitRef::Rev(s) => s,
            GitRef::MergeBase => "merge-base",
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

/// Content of a file - either text lines or binary marker
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FileContent {
    Text { lines: Vec<String> },
    Binary,
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
pub struct FileDiffSummary {
    pub before: Option<PathBuf>,
    pub after: Option<PathBuf>,
}

impl FileDiffSummary {
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
