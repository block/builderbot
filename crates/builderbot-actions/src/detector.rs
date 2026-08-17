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
    pub auto_commit: bool,
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
- autoCommit: boolean (true if action modifies files and should auto-commit)
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
    "autoCommit": false,
    "source": "package.json"
  },
  {
    "name": "Install Lefthook",
    "command": "lefthook install",
    "actionType": "prerun",
    "autoCommit": false,
    "source": "lefthook.yml"
  },
  {
    "name": "Dev",
    "command": "npm run dev",
    "actionType": "run",
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
    "name": "Build",
    "command": "npm run build",
    "actionType": "build",
    "autoCommit": false,
    "source": "package.json"
  },
  {
    "name": "Format",
    "command": "prettier --write .",
    "actionType": "format",
    "autoCommit": true,
    "source": "package.json"
  },
  {
    "name": "Storybook",
    "command": "npm run storybook",
    "actionType": "run",
    "autoCommit": false,
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

/// Parse the AI response and extract suggested actions.
///
/// The response is not guaranteed to be bare JSON: agents wrap the array in a
/// markdown code fence, prefix it with prose, and may emit brackets of their own
/// (`[1]`, glob patterns, transcript markers) before the answer. So candidate
/// `[` positions are scanned from the **end** of the response — the agent's
/// final array wins — and each candidate is prefix-parsed, which lets trailing
/// text such as a closing fence be ignored. A candidate only counts if it
/// deserializes into `Vec<SuggestedAction>`, so unrelated arrays are skipped.
fn parse_ai_response(response: &str) -> Result<Vec<SuggestedAction>> {
    for (start, _) in response.rmatch_indices('[') {
        let mut stream = serde_json::Deserializer::from_str(&response[start..])
            .into_iter::<Vec<SuggestedAction>>();
        if let Some(Ok(actions)) = stream.next() {
            return Ok(actions);
        }
    }

    Err(anyhow::anyhow!(
        "Could not find valid JSON array in AI response. Response was: {}",
        response
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ACTION: &str = r#"{"name": "Test", "command": "npm test", "actionType": "check", "autoCommit": false, "source": "package.json"}"#;

    #[test]
    fn parses_array_surrounded_by_prose() {
        let text = format!("Here are some actions:\n[\n  {TEST_ACTION}\n]\nThat's all!");

        let actions = parse_ai_response(&text).expect("array between prose should parse");

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].name, "Test");
    }

    #[test]
    fn parses_bare_array() {
        let text = format!("[{TEST_ACTION}]");

        let actions = parse_ai_response(&text).expect("bare array should parse");

        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn parses_fenced_array_after_tool_transcript() {
        // Regression: one-shot transcripts used to interleave `[Tool: …]` /
        // `[Result: …]` markers, whose leading `[` swallowed the real array.
        let text = format!(
            "I'll explore the project structure.\n\
             [Tool: Terminal]\n\
             [Result: List top-level project files]\n\
             ```json\n\
             [{TEST_ACTION}]\n\
             ```\n"
        );

        let actions = parse_ai_response(&text).expect("fenced array after markers should parse");

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].command, "npm test");
    }

    #[test]
    fn prefers_the_final_array_over_earlier_ones() {
        let text = format!(
            "Example of the shape I will return:\n\
             [{{\"name\": \"Example\", \"command\": \"echo hi\", \"actionType\": \"run\", \"autoCommit\": false, \"source\": \"README.md\"}}]\n\
             Here is the real answer:\n\
             [{TEST_ACTION}]"
        );

        let actions = parse_ai_response(&text).expect("last valid array should win");

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].name, "Test");
    }

    #[test]
    fn skips_arrays_that_are_not_actions() {
        let text = format!("Candidates: [\"justfile\", \"package.json\"]\n[{TEST_ACTION}]\n[1, 2]");

        let actions = parse_ai_response(&text).expect("non-action arrays should be skipped");

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].source, "package.json");
    }

    #[test]
    fn errors_when_no_action_array_is_present() {
        let err = parse_ai_response("I could not find any build files. [Tool: Terminal]")
            .expect_err("responses without an action array should fail");

        assert!(err.to_string().contains("Could not find valid JSON array"));
    }
}
