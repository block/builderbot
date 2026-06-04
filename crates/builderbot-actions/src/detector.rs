//! AI-powered action detection
//!
//! This module uses an AI agent to analyze project structure and suggest
//! relevant actions (linting, testing, formatting, etc.) based on common
//! patterns in build files (justfile, Makefile, package.json, etc.).
//!
//! The agent is responsible for exploring the project files itself – either
//! on the local filesystem (when a clone exists) or via the `gh` CLI (when
//! no local clone is available).

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::models::ActionType;

/// Trait for AI providers that can generate text from prompts
#[async_trait]
pub trait AiProvider: Send + Sync {
    async fn prompt(&self, prompt: String) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Suggested action
// ---------------------------------------------------------------------------

/// A suggested action that was detected
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedAction {
    pub name: String,
    pub command: String,
    pub action_type: ActionType,
    pub source: String, // e.g., "justfile", "Makefile", "package.json"
}

// ---------------------------------------------------------------------------
// File exploration mode
// ---------------------------------------------------------------------------

/// How the agent should explore the project files.
pub enum FileExplorationMode {
    /// The agent has a local checkout and can use normal shell commands
    /// (`ls`, `cat`, `find`, etc.) to explore files.
    Local {
        /// The working directory path (needed to check git config).
        working_dir: std::path::PathBuf,
    },
    /// No local clone exists. The agent should use `gh api` to explore
    /// the repository on GitHub.
    GitHub {
        /// Full repo identifier, e.g. `"squareup/builderbot"`.
        repo: String,
        /// Optional subdirectory to scope detection to.
        subpath: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Prompt templates
// ---------------------------------------------------------------------------

/// Core instructions shared by both local and GitHub modes.
const DETECTION_PROMPT_CORE: &str = r#"You are analyzing a project to detect available development actions (build, test, lint, format commands).

Your task:
1. Explore the project structure to find build/config files
2. Read relevant files to understand available commands
3. Return a JSON array of suggested actions

IMPORTANT FILES TO LOOK FOR:
- justfile / Justfile (just command runner)
- Makefile / makefile
- package.json (npm/yarn/pnpm scripts)
- Cargo.toml (Rust)
- pyproject.toml / setup.py (Python)
- tsconfig.json, eslint.config.js, .eslintrc.json, .prettierrc
- lefthook.yml (git hooks)
- Also check subdirectories (1-2 levels deep) for additional justfile/Makefile files

IMPORTANT: Return your final response as valid JSON ONLY. Do not include any explanatory text before or after the JSON.

The response must be a JSON array of action objects. Each action object must have these fields:
- name: string (concise action name, e.g., "Test", "Lint", "Format")
- command: string (exact shell command to run, e.g., "npm test", "just build")
- actionType: string (one of: "prerun", "run", "build", "format", "check", "test", "cleanUp")
- source: string (which file this was detected from, e.g., "package.json", "justfile", "subdir/justfile")

Action type guidelines:
- "prerun": Commands that should run automatically on worktree creation (like "npm install", "yarn", "pnpm install", "lefthook install")
- "build": Commands that compile or build the project (like "npm run build", "cargo build", "just build", "make build")
- "format": Commands that auto-fix code (like "just fmt", "just lint-fix", "prettier --write", "cargo fmt", "ruff format")
- "check": Commands that validate without modifying (like "eslint", "cargo clippy", "mypy")
- "test": Commands that run tests (like "npm test", "cargo test", "pytest")
- "cleanUp": Commands that clean up build artifacts (like "npm run clean", "cargo clean", "rm -rf dist")
- "run": Development servers and other commands (like "npm run dev", "npm start", "just run", "storybook")

Special instructions for lefthook:
{lefthook_instructions}

Special instructions for subdirectory build files:
- If justfile/Justfile/Makefile/makefile files are found in subdirectories, detect actions from them
- Commands will be executed from the same directory you are exploring from
- If you need to run a command from a different directory, include the appropriate "cd <path> && " prefix
- Include the subdirectory path in the source field (e.g., "source": "staged/justfile")

Action ordering (list most important first):
- Primary dev commands should come first (like "dev", "start")
- Commonly used commands next (like "test", "build", "format")
- Utility/secondary commands last (like "storybook", "docs", "clean")

When categorizing actions, examine what each script actually does:
- If a script runs formatters or auto-fixes issues, it's "format" (even if named "lint")
- If a script only validates/checks without modifying files, it's "check"
- Look at the actual commands in justfile/Makefile targets to determine behavior

IMPORTANT: Only suggest actions suitable for development environments. Skip:
- Deploy/production commands (like "deploy", "publish", "release")
- CI/CD specific commands
- Docker/container deployment commands
- Cloud infrastructure commands

Return ONLY a JSON array with detected actions. Example (ordered by importance):
[
  {
    "name": "Install Dependencies",
    "command": "npm install",
    "actionType": "prerun",
    "source": "package.json"
  },
  {
    "name": "Install Lefthook",
    "command": "lefthook install",
    "actionType": "prerun",
    "source": "lefthook.yml"
  },
  {
    "name": "Dev",
    "command": "npm run dev",
    "actionType": "run",
    "source": "package.json"
  },
  {
    "name": "Test",
    "command": "npm test",
    "actionType": "test",
    "source": "package.json"
  },
  {
    "name": "Build",
    "command": "npm run build",
    "actionType": "build",
    "source": "package.json"
  },
  {
    "name": "Format",
    "command": "prettier --write .",
    "actionType": "format",
    "source": "package.json"
  },
  {
    "name": "Storybook",
    "command": "npm run storybook",
    "actionType": "run",
    "source": "package.json"
  }
]"#;

/// Extra instructions for local file exploration.
const LOCAL_EXPLORATION_INSTRUCTIONS: &str = r#"
HOW TO EXPLORE THE PROJECT:
- Use shell commands like `ls`, `cat`, `find`, etc. to explore the project files
- You are running in the project directory, and all commands you generate will also be executed from this same directory
- Start by listing the top-level files, then read relevant build/config files
- Check subdirectories (1-2 levels) for additional build files"#;

/// Extra instructions for GitHub API file exploration.
const GITHUB_EXPLORATION_INSTRUCTIONS: &str = r#"
HOW TO EXPLORE THE PROJECT:
- There is NO local clone of this repository. You must use the `gh` CLI to explore files.
- To list the file tree: `gh api 'repos/{repo}/git/trees/HEAD?recursive=1' --jq '.tree[] | select(.type==\"blob\") | .path'`
- To read a file: `gh api 'repos/{repo}/contents/{path}' --jq '.content' | base64 --decode`
- Start by listing the file tree to find build/config files, then read the relevant ones
- {subpath_instructions}"#;

// ---------------------------------------------------------------------------
// ActionDetector
// ---------------------------------------------------------------------------

/// Action detector that uses an AI provider to detect actions from project files
pub struct ActionDetector {
    provider: Box<dyn AiProvider>,
}

impl ActionDetector {
    /// Create a new action detector with the given AI provider
    pub fn new(provider: Box<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Detect actions from a local project directory using AI.
    ///
    /// This is a convenience wrapper that uses [`FileExplorationMode::Local`].
    pub async fn detect_actions(&self, working_dir: &Path) -> Result<Vec<SuggestedAction>> {
        self.detect_actions_with_mode(FileExplorationMode::Local {
            working_dir: working_dir.to_path_buf(),
        })
        .await
    }

    /// Detect actions using the specified [`FileExplorationMode`].
    pub async fn detect_actions_with_mode(
        &self,
        mode: FileExplorationMode,
    ) -> Result<Vec<SuggestedAction>> {
        let exploration_instructions = match &mode {
            FileExplorationMode::Local { .. } => LOCAL_EXPLORATION_INSTRUCTIONS.to_string(),
            FileExplorationMode::GitHub { repo, subpath } => {
                let subpath_instructions = match subpath {
                    Some(sub) if !sub.is_empty() => {
                        format!(
                            "Focus on files under the `{sub}/` subdirectory, but also check the repo root for top-level config files."
                        )
                    }
                    _ => "Explore from the repository root.".to_string(),
                };
                GITHUB_EXPLORATION_INSTRUCTIONS
                    .replace("{repo}", repo)
                    .replace("{subpath_instructions}", &subpath_instructions)
            }
        };

        // Check if the user has git hook overrides (core.hooksPath).
        // If so, lefthook install is unnecessary since hooks are managed externally.
        let lefthook_instructions = match &mode {
            FileExplorationMode::Local { working_dir } => {
                if has_git_hooks_path_override(working_dir) {
                    "- SKIP lefthook detection entirely. The user has a custom git core.hooksPath configured, \
                     so lefthook install is not needed and should NOT be suggested as an action."
                        .to_string()
                } else {
                    "- If lefthook.yml is present in the project, ALWAYS include \"lefthook install\" as a prerun action\n\
                     - This ensures git hooks are properly installed in each new worktree"
                        .to_string()
                }
            }
            FileExplorationMode::GitHub { .. } => {
                // We can't check git config without a local clone, so we always include
                // lefthook detection and let the agent decide based on whether lefthook.yml exists.
                "- If lefthook.yml is present in the project, ALWAYS include \"lefthook install\" as a prerun action\n\
                 - This ensures git hooks are properly installed in each new worktree"
                    .to_string()
            }
        };

        let prompt = format!(
            "{DETECTION_PROMPT_CORE}\n{exploration_instructions}",
            DETECTION_PROMPT_CORE =
                DETECTION_PROMPT_CORE.replace("{lefthook_instructions}", &lefthook_instructions),
        );

        // Call AI to analyze and suggest actions
        let response = self
            .provider
            .prompt(prompt)
            .await
            .map_err(|e| anyhow::anyhow!("AI detection failed: {}", e))?;

        // Parse the JSON response
        parse_ai_response(&response)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if the repository has a custom git hooks path configured (core.hooksPath).
/// When this is set, the user manages git hooks externally and lefthook install
/// should be skipped.
fn has_git_hooks_path_override(working_dir: &Path) -> bool {
    std::process::Command::new("git")
        .args(["config", "core.hooksPath"])
        .current_dir(working_dir)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

/// Parse the AI response and extract suggested actions
fn parse_ai_response(response: &str) -> Result<Vec<SuggestedAction>> {
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
  {"name": "Test", "command": "npm test", "actionType": "check", "source": "package.json"}
]
That's all!"#;

        let result = extract_json_array(text);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_json_array_clean() {
        let text = r#"[{"name": "Test", "command": "npm test", "actionType": "check", "source": "package.json"}]"#;

        let result = extract_json_array(text);
        assert!(result.is_ok());
    }
}
