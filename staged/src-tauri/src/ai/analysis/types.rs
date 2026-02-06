//! Types for AI interactions and session management.

use serde::{Deserialize, Serialize};

// =============================================================================
// Smart Diff Analysis Types
// =============================================================================

/// Analysis result for a changeset (used by smart diff).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetAnalysis {
    pub summary: String,
    pub key_changes: Vec<String>,
    pub concerns: Vec<String>,
    pub file_annotations: std::collections::HashMap<String, Vec<SmartDiffAnnotation>>,
}

/// Summary portion of changeset analysis (for storage).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesetSummary {
    pub summary: String,
    pub key_changes: Vec<String>,
    pub concerns: Vec<String>,
}

/// Result of AI analysis on a single file's diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartDiffResult {
    pub overview: String,
    pub annotations: Vec<SmartDiffAnnotation>,
}

/// A single AI annotation on a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartDiffAnnotation {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_span: Option<LineSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_span: Option<LineSpan>,
    pub content: String,
    pub category: AnnotationCategory,
}

/// A span of lines (0-indexed, exclusive end).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineSpan {
    pub start: usize,
    pub end: usize,
}

/// Category of AI annotation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationCategory {
    Explanation,
    Warning,
    Suggestion,
    Context,
}
