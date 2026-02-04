//! AI tool discovery and execution via ACP (Agent Client Protocol).
//!
//! This module handles AI-powered diff analysis by communicating with
//! ACP-compatible agents like Goose.

use std::path::Path;

use super::prompt::{build_prompt_with_strategy, FileAnalysisInput, LARGE_FILE_THRESHOLD};
use super::types::ChangesetAnalysis;
use crate::ai::{find_acp_agent, find_acp_agent_by_id, run_acp_prompt, AcpAgent};
use crate::git::{self, DiffSpec, FileContent};

/// Find an available AI agent, optionally by provider ID.
///
/// If `provider` is specified, looks for that specific provider.
/// Otherwise, returns the default (Goose if available, then Claude).
pub fn find_ai_tool(provider: Option<&str>) -> Option<AcpAgent> {
    match provider {
        Some(id) => find_acp_agent_by_id(id),
        None => find_acp_agent(),
    }
}

/// Check if output contains a context window error.
fn detect_context_error(output: &str) -> Option<String> {
    let output_lower = output.to_lowercase();

    let error_patterns = &[
        "context limit reached",
        "context length exceeded",
        "maximum context length exceeded",
        "prompt is too long",
        "input too long",
        "exceeds the maximum number of tokens",
    ];

    for pattern in error_patterns {
        if output_lower.contains(pattern) {
            return Some(
                "Changeset too large for AI analysis. \
                 Try analyzing fewer files or a smaller diff range."
                    .to_string(),
            );
        }
    }
    None
}

/// Parse AI response into ChangesetAnalysis
fn parse_response(response: &str) -> Result<ChangesetAnalysis, String> {
    let response = response.trim();
    let json_str = extract_json(response);

    serde_json::from_str(json_str).map_err(|e| {
        log::error!("Failed to parse response as JSON: {}", e);
        log::error!("Response was:\n{}", response);
        format!("Failed to parse AI response: {}", e)
    })
}

/// Load after content for a file if it's small enough.
/// Returns None for deleted files, binary files, or files exceeding the threshold.
fn load_after_content_if_small(
    repo_path: &Path,
    spec: &DiffSpec,
    file_path: &Path,
) -> Result<(Option<String>, usize), String> {
    let diff = git::get_file_diff(repo_path, spec, file_path)
        .map_err(|e| format!("Failed to get file diff: {}", e))?;

    let (content, line_count) = match &diff.after {
        Some(f) => match &f.content {
            FileContent::Text { lines } => {
                let count = lines.len();
                if count <= LARGE_FILE_THRESHOLD {
                    (Some(lines.join("\n")), count)
                } else {
                    // File too large, skip content but report line count
                    (None, count)
                }
            }
            FileContent::Binary => (None, 0),
        },
        None => (None, 0), // Deleted file
    };

    Ok((content, line_count))
}

/// Analyze a diff using AI via ACP.
///
/// This is the main entry point - it handles:
/// 1. Listing files in the diff
/// 2. Loading unified diffs and after content for each file
/// 3. Building an appropriately-sized prompt (with automatic tier selection)
/// 4. Running AI analysis via ACP
/// 5. Returning the complete result
///
/// The frontend just needs to provide the diff spec and optionally a provider ID.
pub async fn analyze_diff(
    repo_path: &Path,
    spec: &DiffSpec,
    provider: Option<&str>,
) -> Result<ChangesetAnalysis, String> {
    // Find AI agent first (fail fast)
    let agent = find_ai_tool(provider).ok_or_else(|| match provider {
        Some(id) => format!(
            "Provider '{}' not found. Run discover_acp_providers to see available providers.",
            id
        ),
        None => "No AI agent found. Install Goose: https://github.com/block/goose".to_string(),
    })?;

    // List files in the diff
    let files = git::list_diff_files(repo_path, spec)
        .map_err(|e| format!("Failed to list diff files: {}", e))?;

    if files.is_empty() {
        return Err("No files in diff to analyze".to_string());
    }

    // Build inputs for each file
    let mut inputs: Vec<FileAnalysisInput> = Vec::new();

    for file_summary in &files {
        let file_path = file_summary.path();
        let path_str = file_path.to_string_lossy().to_string();

        // Get unified diff
        let diff = git::get_unified_diff(repo_path, spec, file_path)
            .map_err(|e| format!("Failed to get diff for {}: {}", path_str, e))?;

        // Load after content if small enough
        let (after_content, after_line_count) =
            load_after_content_if_small(repo_path, spec, file_path)?;

        // Determine file status
        let is_new_file = file_summary.is_added();
        let is_deleted = file_summary.is_deleted();

        // Skip binary files (no diff and no content)
        if diff.is_empty() && after_content.is_none() && !is_new_file && !is_deleted {
            // Check if it's actually binary by looking at the file
            let file_diff = git::get_file_diff(repo_path, spec, file_path).ok();
            let is_binary = file_diff.is_some_and(|d| {
                matches!(
                    d.after.as_ref().map(|f| &f.content),
                    Some(FileContent::Binary)
                ) || matches!(
                    d.before.as_ref().map(|f| &f.content),
                    Some(FileContent::Binary)
                )
            });
            if is_binary {
                continue;
            }
        }

        inputs.push(FileAnalysisInput {
            path: path_str,
            diff,
            after_content,
            is_new_file,
            is_deleted,
            after_line_count,
        });
    }

    if inputs.is_empty() {
        return Err("No text files to analyze (all binary?)".to_string());
    }

    // Build prompt with automatic tier selection
    let (prompt, strategy) = build_prompt_with_strategy(&inputs);

    log::info!("=== DIFF ANALYSIS (ACP) ===");
    log::info!("Files: {}", inputs.len());
    log::info!("Strategy: {:?}", strategy);
    log::info!("Using: {}", agent.name());
    log::debug!("Prompt:\n{}", prompt);

    // Run the prompt via ACP
    let response = run_acp_prompt(&agent, repo_path, &prompt).await?;

    // Check for context window errors
    if let Some(error_msg) = detect_context_error(&response) {
        return Err(error_msg);
    }

    log::debug!("Raw response:\n{}", response);

    parse_response(&response)
}

fn extract_json(response: &str) -> &str {
    // Check for ```json ... ``` pattern
    if let Some(start) = response.find("```json") {
        let after_fence = &response[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim();
        }
    }

    // Check for ``` ... ``` pattern (no language)
    if let Some(start) = response.find("```") {
        let after_fence = &response[start + 3..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim();
        }
    }

    // Try to find JSON object directly
    if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            return &response[start..=end];
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_plain() {
        let input =
            r#"{"summary": "test", "key_changes": [], "concerns": [], "file_annotations": {}}"#;
        assert_eq!(extract_json(input), input);
    }

    #[test]
    fn test_extract_json_with_fence() {
        let input = r#"Here's the analysis:
```json
{"summary": "test", "key_changes": [], "concerns": [], "file_annotations": {}}
```"#;
        assert_eq!(
            extract_json(input),
            r#"{"summary": "test", "key_changes": [], "concerns": [], "file_annotations": {}}"#
        );
    }

    #[test]
    fn test_detect_context_error() {
        assert!(detect_context_error("Error: context limit reached").is_some());
        assert!(detect_context_error("Error: prompt is too long").is_some());
        assert!(detect_context_error("Normal output here").is_none());
        // Should NOT match general mentions of "context window" in analysis
        assert!(detect_context_error("This code handles context window errors").is_none());
    }
}
