//! Prompt template for AI diff analysis.
//!
//! Implements a tiered strategy for prompt construction:
//! - Tier 1: Full AFTER content + unified diff (default, for smaller changesets)
//! - Tier 2: Unified diff only (fallback for large changesets)
//!
//! Per-file rule: Files > 1,000 lines get diff-only treatment even in Tier 1.

/// Threshold for individual files: above this, only include diff (no full content)
pub const LARGE_FILE_THRESHOLD: usize = 1000;

/// Threshold for total prompt: above this, switch to diff-only mode for all files
pub const TIER1_MAX_LINES: usize = 10000;

/// Maximum prompt size in bytes for Codex (10MB limit from API)
/// We use 9MB to leave some buffer for system context
pub const CODEX_MAX_BYTES: usize = 9 * 1024 * 1024;

/// Strategy used for prompt construction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStrategy {
    /// Tier 1: diff + after content for small files
    FullContext,
    /// Tier 2: diff only for all files
    DiffOnly,
}

/// Input for analyzing a single file
#[derive(Debug, Clone)]
pub struct FileAnalysisInput {
    /// File path
    pub path: String,
    /// Unified diff output
    pub diff: String,
    /// Full "after" content (None if file too large, deleted, or binary)
    pub after_content: Option<String>,
    /// Whether this is a new file
    pub is_new_file: bool,
    /// Whether this is a deleted file
    pub is_deleted: bool,
    /// Line count of after content (for logging)
    pub after_line_count: usize,
}

/// Format content with line numbers for the AI to reference.
fn format_with_line_numbers(content: &str) -> String {
    content
        .lines()
        .enumerate()
        .map(|(i, line)| format!("{i:4} | {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Count lines in diff and content for a file input
fn count_file_lines(input: &FileAnalysisInput, include_content: bool) -> usize {
    let diff_lines = input.diff.lines().count();
    let content_lines = if include_content {
        input
            .after_content
            .as_ref()
            .map_or(0, |c| c.lines().count())
    } else {
        0
    };
    diff_lines + content_lines
}

/// Build a prompt with automatic tier selection based on size.
///
/// Returns the prompt string and the strategy that was used.
///
/// For Codex, enforces a stricter byte limit (9MB) to avoid API errors.
pub fn build_prompt_with_strategy(files: &[FileAnalysisInput]) -> (String, PromptStrategy) {
    build_prompt_with_strategy_for_provider(files, None)
}

/// Build a prompt with automatic tier selection based on size and provider.
///
/// If provider is "codex", uses stricter size limits to avoid API errors.
pub fn build_prompt_with_strategy_for_provider(
    files: &[FileAnalysisInput],
    provider: Option<&str>,
) -> (String, PromptStrategy) {
    let is_codex = provider == Some("codex");

    // First, try Tier 1 (full context for small files)
    let tier1_lines: usize = files
        .iter()
        .map(|f| {
            let include_content =
                f.after_content.is_some() && f.after_line_count <= LARGE_FILE_THRESHOLD;
            count_file_lines(f, include_content)
        })
        .sum();

    if tier1_lines <= TIER1_MAX_LINES {
        // Tier 1: diff + after content for small files
        let prompt = build_tier1_prompt(files);

        // For Codex, check byte size and fall back to Tier 2 if too large
        if is_codex && prompt.len() > CODEX_MAX_BYTES {
            log::info!(
                "Prompt too large for Codex ({} bytes, limit {}), using diff-only mode",
                prompt.len(),
                CODEX_MAX_BYTES
            );
            let prompt = build_tier2_prompt(files);
            return (prompt, PromptStrategy::DiffOnly);
        }

        return (prompt, PromptStrategy::FullContext);
    }

    // Tier 2: diff only for all files
    log::info!("Changeset too large for full context ({tier1_lines} lines), using diff-only mode");
    let prompt = build_tier2_prompt(files);

    // Note: For Codex, byte-size validation for Tier 2 happens in runner.rs so
    // we can surface a clear error to the UI. There's no smaller tier here.
    (prompt, PromptStrategy::DiffOnly)
}

/// Build Tier 1 prompt: diff + after content for small files
fn build_tier1_prompt(files: &[FileAnalysisInput]) -> String {
    let mut file_sections = String::new();

    for input in files {
        file_sections.push_str(&format!("\n## File: {}", input.path));

        // Add size note for large files
        if input.after_line_count > LARGE_FILE_THRESHOLD {
            file_sections.push_str(&format!(" ({} lines - diff only)", input.after_line_count));
        }
        file_sections.push_str("\n\n");

        // Always include diff
        file_sections.push_str("### Diff:\n");
        if input.diff.is_empty() {
            if input.is_new_file {
                file_sections.push_str("(new file)\n\n");
            } else if input.is_deleted {
                file_sections.push_str("(file deleted)\n\n");
            } else {
                file_sections.push_str("(no changes)\n\n");
            }
        } else {
            file_sections.push_str("```diff\n");
            file_sections.push_str(&input.diff);
            if !input.diff.ends_with('\n') {
                file_sections.push('\n');
            }
            file_sections.push_str("```\n\n");
        }

        // Include full content for small files (not deleted, not too large)
        if let Some(ref content) = input.after_content {
            if input.after_line_count <= LARGE_FILE_THRESHOLD {
                file_sections.push_str("### Full Content (after):\n```\n");
                file_sections.push_str(&format_with_line_numbers(content));
                file_sections.push_str("\n```\n\n");
            }
        }
    }

    format!(
        r#"{SYSTEM_PROMPT_TIER1}

# Changeset ({file_count} files)
{file_sections}

{OUTPUT_FORMAT}"#,
        SYSTEM_PROMPT_TIER1 = SYSTEM_PROMPT_TIER1,
        file_count = files.len(),
        file_sections = file_sections,
        OUTPUT_FORMAT = OUTPUT_FORMAT,
    )
}

/// Build Tier 2 prompt: diff only for all files
fn build_tier2_prompt(files: &[FileAnalysisInput]) -> String {
    let mut file_sections = String::new();

    for input in files {
        file_sections.push_str(&format!("\n## File: {}\n\n", input.path));

        file_sections.push_str("### Diff:\n");
        if input.diff.is_empty() {
            if input.is_new_file {
                file_sections.push_str("(new file)\n\n");
            } else if input.is_deleted {
                file_sections.push_str("(file deleted)\n\n");
            } else {
                file_sections.push_str("(no changes)\n\n");
            }
        } else {
            file_sections.push_str("```diff\n");
            file_sections.push_str(&input.diff);
            if !input.diff.ends_with('\n') {
                file_sections.push('\n');
            }
            file_sections.push_str("```\n\n");
        }
    }

    format!(
        r#"{SYSTEM_PROMPT_TIER2}

# Changeset ({file_count} files)
{file_sections}

{OUTPUT_FORMAT}"#,
        SYSTEM_PROMPT_TIER2 = SYSTEM_PROMPT_TIER2,
        file_count = files.len(),
        file_sections = file_sections,
        OUTPUT_FORMAT = OUTPUT_FORMAT,
    )
}

const SYSTEM_PROMPT_TIER1: &str = r#"You are a code review assistant analyzing a changeset.

For each file you see:
- A unified diff showing exactly what changed
- For smaller files: the complete "after" content with line numbers for full context
- For large files (>1000 lines): diff only, marked as such

Use the diff to understand what changed. Use the full content (when available) to understand the broader context.

Provide:
1. A high-level summary of what this changeset accomplishes
2. Key changes organized by theme (2-5 bullet points)
3. Any concerns worth noting (0-3 items, empty if none)
4. Annotations on specific code sections that deserve commentary

**Important guidelines**:
- Annotations should tell the story of the change, not exhaustively document every line
- Focus on what matters: the "why", potential issues, non-obvious implications
- It's fine to have no annotations for trivial or self-explanatory files
- Line numbers in annotations reference the AFTER content (0-indexed, from the numbered listing)"#;

const SYSTEM_PROMPT_TIER2: &str = r#"You are a code review assistant analyzing a large changeset.

Due to size, you see unified diffs only (no full file content). Focus your analysis on:
- What the diffs reveal about intent
- Potential issues visible in the changed code
- Cross-file patterns in the changes

Provide:
1. A high-level summary of what this changeset accomplishes
2. Key changes organized by theme (2-5 bullet points)
3. Any concerns worth noting (0-3 items, empty if none)
4. Annotations on specific code sections that deserve commentary

**Important guidelines**:
- Annotations should tell the story of the change, not exhaustively document every line
- Focus on what matters: the "why", potential issues, non-obvious implications
- It's fine to have no annotations for trivial or self-explanatory files
- Line numbers in annotations should reference the new file line numbers shown in the diff (the + lines)"#;

const OUTPUT_FORMAT: &str = r#"## Output Format

Respond with ONLY valid JSON matching this structure (no markdown code fences, no other text):

{
  "summary": "2-3 sentence high-level summary of what this changeset accomplishes",
  "key_changes": [
    "First major change or theme",
    "Second major change or theme"
  ],
  "concerns": [
    "Any potential issue or area needing careful review"
  ],
  "file_annotations": {
    "path/to/file.rs": [
      {
        "id": "1",
        "file_path": "path/to/file.rs",
        "before_span": {"start": 8, "end": 15},
        "before_description": "Previously handled errors by panicking",
        "after_span": {"start": 10, "end": 20},
        "content": "Your commentary on this section",
        "category": "explanation"
      }
    ],
    "path/to/other.ts": []
  }
}

Rules:
- "summary": Brief overview suitable for a PR description
- "key_changes": 2-5 bullet points grouping related changes
- "concerns": 0-3 potential issues (empty array if none)
- "file_annotations": Object with file paths as keys, arrays of annotations as values
  - Include ALL files from the changeset as keys (use empty array [] if no annotations needed)
  - "id": Unique across ALL annotations (use "1", "2", "3", etc.)
  - "file_path": Must match the key exactly
  - "before_span": Line range in BEFORE content (0-indexed, exclusive end). Omit if only about new code.
  - "before_description": When before_span is provided, describe what the old code was doing (1 sentence). Required if before_span is set.
  - "after_span": Line range in AFTER content (0-indexed, exclusive end). Omit if only about deleted code.
  - "content": Your commentary (1-3 sentences)
  - "category": One of "explanation", "warning", "suggestion", "context""#;

// Keep the old function for backward compatibility during transition
// TODO: Remove once runner.rs is updated
/// Build the prompt for analyzing an entire changeset with full file contents.
#[allow(dead_code)]
pub fn build_unified_changeset_prompt(files: &[(&str, &str, &str)]) -> String {
    let mut file_sections = String::new();

    for (path, before, after) in files {
        file_sections.push_str(&format!("\n## File: {path}\n\n"));

        if before.is_empty() {
            file_sections.push_str("### BEFORE:\n(new file - no previous content)\n\n");
        } else {
            file_sections.push_str("### BEFORE:\n```\n");
            file_sections.push_str(&format_with_line_numbers(before));
            file_sections.push_str("\n```\n\n");
        }

        if after.is_empty() {
            file_sections.push_str("### AFTER:\n(deleted file - no new content)\n\n");
        } else {
            file_sections.push_str("### AFTER:\n```\n");
            file_sections.push_str(&format_with_line_numbers(after));
            file_sections.push_str("\n```\n\n");
        }
    }

    format!(
        r#"{SYSTEM_PROMPT_TIER1}

# Changeset ({file_count} files)
{file_sections}

{OUTPUT_FORMAT}"#,
        SYSTEM_PROMPT_TIER1 = SYSTEM_PROMPT_TIER1,
        file_count = files.len(),
        file_sections = file_sections,
        OUTPUT_FORMAT = OUTPUT_FORMAT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oversized_file_input() -> FileAnalysisInput {
        let oversized_content = "a".repeat(CODEX_MAX_BYTES + 1024);

        FileAnalysisInput {
            path: "src/huge.rs".to_string(),
            diff: "@@ -1,1 +1,1 @@\n-old\n+new".to_string(),
            after_content: Some(oversized_content),
            is_new_file: false,
            is_deleted: false,
            after_line_count: 1,
        }
    }

    #[test]
    fn test_build_prompt_small_changeset() {
        let files = vec![FileAnalysisInput {
            path: "src/main.rs".to_string(),
            diff: "@@ -1,3 +1,3 @@\n fn main() {\n-    old();\n+    new();\n }".to_string(),
            after_content: Some("fn main() {\n    new();\n}".to_string()),
            is_new_file: false,
            is_deleted: false,
            after_line_count: 3,
        }];

        let (prompt, strategy) = build_prompt_with_strategy(&files);

        assert_eq!(strategy, PromptStrategy::FullContext);
        assert!(prompt.contains("File: src/main.rs"));
        assert!(prompt.contains("### Diff:"));
        assert!(prompt.contains("### Full Content (after):"));
        assert!(prompt.contains("fn main()"));
    }

    #[test]
    fn test_build_prompt_large_file_excluded() {
        let large_content = (0..1500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let files = vec![FileAnalysisInput {
            path: "src/large.rs".to_string(),
            diff: "@@ -1,3 +1,3 @@\n context\n-old\n+new".to_string(),
            after_content: Some(large_content),
            is_new_file: false,
            is_deleted: false,
            after_line_count: 1500,
        }];

        let (prompt, strategy) = build_prompt_with_strategy(&files);

        assert_eq!(strategy, PromptStrategy::FullContext);
        assert!(prompt.contains("1500 lines - diff only"));
        assert!(!prompt.contains("### Full Content (after):"));
    }

    #[test]
    fn test_build_prompt_tier2_fallback() {
        // Create enough files to exceed TIER1_MAX_LINES
        let files: Vec<FileAnalysisInput> = (0..50)
            .map(|i| {
                let content = (0..300)
                    .map(|j| format!("line {j} in file {i}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                FileAnalysisInput {
                    path: format!("src/file{i}.rs"),
                    diff: format!(
                        "@@ -1,100 +1,100 @@\n{}",
                        (0..100).map(|_| "+line").collect::<Vec<_>>().join("\n")
                    ),
                    after_content: Some(content),
                    is_new_file: false,
                    is_deleted: false,
                    after_line_count: 300,
                }
            })
            .collect();

        let (prompt, strategy) = build_prompt_with_strategy(&files);

        assert_eq!(strategy, PromptStrategy::DiffOnly);
        assert!(prompt.contains("unified diffs only"));
        assert!(!prompt.contains("### Full Content (after):"));
    }

    #[test]
    fn test_build_prompt_new_file() {
        let files = vec![FileAnalysisInput {
            path: "src/new.rs".to_string(),
            diff: "@@ -0,0 +1,3 @@\n+fn new() {\n+    code();\n+}".to_string(),
            after_content: Some("fn new() {\n    code();\n}".to_string()),
            is_new_file: true,
            is_deleted: false,
            after_line_count: 3,
        }];

        let (prompt, strategy) = build_prompt_with_strategy(&files);

        assert_eq!(strategy, PromptStrategy::FullContext);
        assert!(prompt.contains("### Diff:"));
        assert!(prompt.contains("### Full Content (after):"));
    }

    #[test]
    fn test_build_prompt_deleted_file() {
        let files = vec![FileAnalysisInput {
            path: "src/old.rs".to_string(),
            diff: "@@ -1,3 +0,0 @@\n-fn old() {\n-    code();\n-}".to_string(),
            after_content: None,
            is_new_file: false,
            is_deleted: true,
            after_line_count: 0,
        }];

        let (prompt, strategy) = build_prompt_with_strategy(&files);

        assert_eq!(strategy, PromptStrategy::FullContext);
        assert!(prompt.contains("### Diff:"));
        assert!(!prompt.contains("### Full Content (after):"));
    }

    #[test]
    fn test_codex_large_prompt_falls_back_to_tier2() {
        let files = vec![oversized_file_input()];

        let (prompt, strategy) = build_prompt_with_strategy_for_provider(&files, Some("codex"));

        assert_eq!(strategy, PromptStrategy::DiffOnly);
        assert!(prompt.contains("unified diffs only"));
        assert!(!prompt.contains("### Full Content (after):"));
    }

    #[test]
    fn test_non_codex_large_prompt_keeps_tier1() {
        let files = vec![oversized_file_input()];

        let (prompt, strategy) = build_prompt_with_strategy_for_provider(&files, Some("claude"));

        assert_eq!(strategy, PromptStrategy::FullContext);
        assert!(prompt.len() > CODEX_MAX_BYTES);
        assert!(prompt.contains("### Full Content (after):"));
    }
}
