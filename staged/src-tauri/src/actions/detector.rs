//! AI-powered action detection
//!
//! This module uses an AI model to analyze project structure and suggest
//! relevant actions (linting, testing, formatting, etc.) based on common
//! patterns in build files (justfile, Makefile, package.json, etc.).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::ai::{find_acp_agent, run_acp_prompt_raw};
use crate::store::ActionType;

/// A suggested action that was detected
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedAction {
    pub name: String,
    pub command: String,
    pub action_type: ActionType,
    pub auto_commit: bool,
    pub source: String, // e.g., "justfile", "Makefile", "package.json"
}

/// System prompt for AI action detection
const DETECTION_PROMPT_TEMPLATE: &str = r#"You are analyzing a project directory to detect available actions (build, test, lint, format commands).

Analyze the project structure and suggest actions based on the files present.

IMPORTANT: Return your response as valid JSON ONLY. Do not include any explanatory text before or after the JSON.

The response must be a JSON array of action objects. Each action object must have these fields:
- name: string (concise action name, e.g., "Test", "Lint", "Format")
- command: string (exact shell command to run, e.g., "npm test", "just build")
- actionType: string (one of: "prerun", "run", "build", "format", "check", "test", "cleanUp")
- autoCommit: boolean (true if action modifies files and should auto-commit)
- source: string (which file this was detected from, e.g., "package.json", "justfile")

Action type guidelines:
- "prerun": Commands that should run automatically on worktree creation (like "npm install", "yarn", "pnpm install")
- "build": Commands that compile or build the project (like "npm run build", "cargo build", "just build", "make build")
- "format": Commands that auto-fix code (like "just fmt", "just lint-fix", "prettier --write", "cargo fmt", "ruff format")
- "check": Commands that validate without modifying (like "eslint", "cargo clippy", "mypy")
- "test": Commands that run tests (like "npm test", "cargo test", "pytest")
- "cleanUp": Commands that clean up build artifacts (like "npm run clean", "cargo clean", "rm -rf dist")
- "run": Other commands (like "just dev", "npm run dev", "npm start")

When categorizing actions, examine what each script actually does:
- If a script runs formatters or auto-fixes issues, it's "format" (even if named "lint")
- If a script only validates/checks without modifying files, it's "check"
- Look at the actual commands in justfile/Makefile targets to determine behavior

IMPORTANT: Only suggest actions suitable for development environments. Skip:
- Deploy/production commands (like "deploy", "publish", "release")
- CI/CD specific commands
- Docker/container deployment commands
- Cloud infrastructure commands

Project directory contents:
{file_list}

Relevant file contents:
{file_contents}

Return ONLY a JSON array with detected actions. Example:
[
  {
    "name": "Install Dependencies",
    "command": "npm install",
    "actionType": "prerun",
    "autoCommit": false,
    "source": "package.json"
  },
  {
    "name": "Test",
    "command": "npm test",
    "actionType": "test",
    "autoCommit": false,
    "source": "package.json"
  },
  {
    "name": "Format",
    "command": "just fmt",
    "actionType": "format",
    "autoCommit": true,
    "source": "justfile"
  }
]"#;

/// Detect actions from a project repository using AI
pub async fn detect_actions(
    repo_path: &Path,
    subpath: Option<&str>,
) -> Result<Vec<SuggestedAction>> {
    let working_dir = if let Some(sp) = subpath {
        repo_path.join(sp)
    } else {
        repo_path.to_path_buf()
    };

    // Find an available ACP agent
    let agent = find_acp_agent()
        .ok_or_else(|| anyhow::anyhow!("No AI agent available (goose or claude-code-acp). Please install an ACP-compatible agent to use action detection."))?;

    // Collect information about the project
    let file_list = collect_file_list(&working_dir)?;
    let file_contents = collect_relevant_files(&working_dir)?;

    // Build the prompt
    let prompt = DETECTION_PROMPT_TEMPLATE
        .replace("{file_list}", &file_list)
        .replace("{file_contents}", &file_contents);

    // Call AI to analyze and suggest actions
    let response = run_acp_prompt_raw(&agent, &working_dir, &prompt)
        .await
        .map_err(|e| anyhow::anyhow!("AI detection failed: {}", e))?;

    // Parse the JSON response
    parse_ai_response(&response)
}

/// Collect a list of files in the directory
fn collect_file_list(dir: &Path) -> Result<String> {
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();

                // Skip hidden files and common directories
                if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
                    continue;
                }

                if file_type.is_file() {
                    files.push(name_str.to_string());
                } else if file_type.is_dir() {
                    files.push(format!("{}/", name_str));
                }
            }
        }
    }

    files.sort();
    Ok(files.join("\n"))
}

/// Collect contents of relevant build/config files
fn collect_relevant_files(dir: &Path) -> Result<String> {
    let relevant_files = [
        "package.json",
        "justfile",
        "Justfile",
        "Makefile",
        "makefile",
        "Cargo.toml",
        "pyproject.toml",
        "setup.py",
        "tsconfig.json",
        ".eslintrc.json",
        ".eslintrc.js",
        "eslint.config.js",
        ".prettierrc",
        ".prettierrc.json",
    ];

    let mut contents = Vec::new();

    for file_name in &relevant_files {
        let file_path = dir.join(file_name);
        if file_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                // Limit file size to avoid token overflow
                let truncated = if content.len() > 4000 {
                    format!("{}... (truncated)", &content[..4000])
                } else {
                    content
                };
                contents.push(format!("=== {} ===\n{}\n", file_name, truncated));
            }
        }
    }

    if contents.is_empty() {
        Ok("No relevant build files found.".to_string())
    } else {
        Ok(contents.join("\n"))
    }
}

/// Parse the AI response and extract suggested actions
fn parse_ai_response(response: &str) -> Result<Vec<SuggestedAction>> {
    // Try to extract JSON from the response
    // AI might include explanatory text, so we need to find the JSON array
    let json_str = extract_json_array(response)?;

    let actions: Vec<SuggestedAction> = serde_json::from_str(&json_str).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse AI response as JSON: {}. Response was: {}",
            e,
            json_str
        )
    })?;

    Ok(actions)
}

/// Extract JSON array from AI response that might contain extra text
fn extract_json_array(text: &str) -> Result<String> {
    // First try to parse the entire response as JSON
    if text.trim().starts_with('[') && serde_json::from_str::<serde_json::Value>(text).is_ok() {
        return Ok(text.to_string());
    }

    // Look for JSON array in the text
    if let Some(start) = text.find('[') {
        if let Some(end) = text.rfind(']') {
            if end > start {
                let json_str = &text[start..=end];
                // Validate it's valid JSON
                if serde_json::from_str::<serde_json::Value>(json_str).is_ok() {
                    return Ok(json_str.to_string());
                }
            }
        }
    }

    Err(anyhow::anyhow!(
        "Could not find valid JSON array in AI response. Response was: {}",
        text
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_array() {
        let text = r#"Here are some actions:
[
  {"name": "Test", "command": "npm test", "actionType": "check", "autoCommit": false, "source": "package.json"}
]
That's all!"#;

        let result = extract_json_array(text);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_json_array_clean() {
        let text = r#"[{"name": "Test", "command": "npm test", "actionType": "check", "autoCommit": false, "source": "package.json"}]"#;

        let result = extract_json_array(text);
        assert!(result.is_ok());
    }
}
