//! Core data types for the diff viewer.
//!
//! These types represent the minimal information needed to display diffs
//! and track reviews. The design prioritizes simplicity and statelessness.

use serde::{Deserialize, Serialize};

use super::git::WORKDIR;

/// Identifies a diff between two repository states.
///
/// - `before`: A ref (branch name, tag), SHA, or "HEAD"
/// - `after`: A ref, SHA, or "WORKDIR" for the working tree
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

/// A file with its path and content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub content: FileContent,
}

/// The diff for a single file between two states.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    /// The file before the change (None if file was added)
    pub before: Option<File>,
    /// The file after the change (None if file was deleted)
    pub after: Option<File>,
    /// Alignments mapping regions between before/after for scroll sync and display
    pub alignments: Vec<Alignment>,
}

impl FileDiff {
    /// Returns the primary path for this diff (prefers after, falls back to before).
    pub fn path(&self) -> &str {
        self.after
            .as_ref()
            .map(|f| f.path.as_str())
            .or_else(|| self.before.as_ref().map(|f| f.path.as_str()))
            .unwrap_or("")
    }

    /// Returns the kind of change this file underwent.
    pub fn change_kind(&self) -> ChangeKind {
        match (&self.before, &self.after) {
            (None, Some(_)) => ChangeKind::Added,
            (Some(_), None) => ChangeKind::Deleted,
            (Some(_), Some(_)) => ChangeKind::Modified,
            (None, None) => ChangeKind::Modified, // shouldn't happen
        }
    }

    /// Returns true if this is a rename (before and after paths differ).
    pub fn is_rename(&self) -> bool {
        match (&self.before, &self.after) {
            (Some(b), Some(a)) => b.path != a.path,
            _ => false,
        }
    }

    /// Returns true if either side is binary.
    pub fn is_binary(&self) -> bool {
        matches!(
            &self.before,
            Some(File {
                content: FileContent::Binary,
                ..
            })
        ) || matches!(
            &self.after,
            Some(File {
                content: FileContent::Binary,
                ..
            })
        )
    }
}

/// The type of change a file underwent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// Content of a file at a specific state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FileContent {
    Text { lines: Vec<String> },
    Binary,
}

impl FileContent {
    /// Create text content from a string, splitting into lines.
    pub fn from_text(content: &str) -> Self {
        let lines: Vec<String> = content.lines().map(String::from).collect();
        Self::Text { lines }
    }

    /// Check if content appears to be binary.
    pub fn is_binary_data(bytes: &[u8]) -> bool {
        // Check for null bytes in first 8KB (common heuristic)
        let check_len = bytes.len().min(8192);
        bytes[..check_len].contains(&0)
    }

    /// Get lines if this is text content.
    pub fn lines(&self) -> &[String] {
        match self {
            FileContent::Text { lines } => lines,
            FileContent::Binary => &[],
        }
    }
}

/// An alignment between a region in the before file and a region in the after file.
///
/// Alignments exhaustively partition both files - every line belongs to exactly
/// one alignment. Used for scroll synchronization and drawing connectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alignment {
    pub before: Span,
    pub after: Span,
    /// True if this region contains changes (content differs between before/after)
    pub changed: bool,
}

/// A contiguous range of lines (0-indexed, exclusive end).
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
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_id_working_tree() {
        let working = DiffId::new("HEAD", WORKDIR);
        assert!(working.is_working_tree());

        let historical = DiffId::new("main", "feature");
        assert!(!historical.is_working_tree());
    }

    #[test]
    fn test_change_kind() {
        let added = FileDiff {
            before: None,
            after: Some(File {
                path: "new.txt".into(),
                content: FileContent::Text { lines: vec![] },
            }),
            alignments: vec![],
        };
        assert_eq!(added.change_kind(), ChangeKind::Added);

        let deleted = FileDiff {
            before: Some(File {
                path: "old.txt".into(),
                content: FileContent::Text { lines: vec![] },
            }),
            after: None,
            alignments: vec![],
        };
        assert_eq!(deleted.change_kind(), ChangeKind::Deleted);

        let modified = FileDiff {
            before: Some(File {
                path: "changed.txt".into(),
                content: FileContent::Text { lines: vec![] },
            }),
            after: Some(File {
                path: "changed.txt".into(),
                content: FileContent::Text { lines: vec![] },
            }),
            alignments: vec![],
        };
        assert_eq!(modified.change_kind(), ChangeKind::Modified);
    }

    #[test]
    fn test_binary_detection() {
        assert!(FileContent::is_binary_data(&[0x00, 0x01, 0x02]));
        assert!(!FileContent::is_binary_data(b"hello world"));
    }

    #[test]
    fn test_is_rename() {
        let rename = FileDiff {
            before: Some(File {
                path: "old_name.txt".into(),
                content: FileContent::Text { lines: vec![] },
            }),
            after: Some(File {
                path: "new_name.txt".into(),
                content: FileContent::Text { lines: vec![] },
            }),
            alignments: vec![],
        };
        assert!(rename.is_rename());

        let not_rename = FileDiff {
            before: Some(File {
                path: "same.txt".into(),
                content: FileContent::Text { lines: vec![] },
            }),
            after: Some(File {
                path: "same.txt".into(),
                content: FileContent::Text { lines: vec![] },
            }),
            alignments: vec![],
        };
        assert!(!not_rename.is_rename());
    }
}
