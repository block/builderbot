# acp-client

A simple, reusable Rust client for communicating with ACP (Agent Client Protocol) compatible agents like Goose and Claude Code.

## Overview

This crate provides a straightforward way to:
- Discover installed ACP agents on the system
- Send one-shot prompts to agents
- Get text responses from agents
- Handle the ACP protocol handshake automatically

The implementation is extracted from the working ACP code in the Staged codebase and designed to be framework-agnostic and reusable across projects.

## Features

- **Agent Discovery**: Automatically finds installed ACP agents (Goose, Claude Code, Codex)
- **Simple API**: Send a prompt, get a response
- **Protocol Handling**: Uses the official `agent-client-protocol` SDK
- **Cross-Platform**: Works on macOS, Linux, and Windows
- **Shell Integration**: Discovers agents using login shell PATH

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
acp-client = { path = "../acp-client" }
tokio = { version = "1", features = ["rt", "macros"] }
```

### Basic Example

```rust
use acp_client::{find_acp_agent, run_acp_prompt};
use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Find an available ACP agent
    let agent = find_acp_agent()
        .ok_or_else(|| anyhow::anyhow!("No ACP agent found. Install goose or claude-agent-acp."))?;

    // Send a prompt and get a response
    let response = run_acp_prompt(
        &agent,
        Path::new("."),
        "What files are in this directory?"
    ).await?;

    println!("Agent response: {}", response);
    Ok(())
}
```

### Discovery Example

```rust
use acp_client::{discover_acp_providers, find_acp_agent_by_id};

// Discover all available agents
let providers = discover_acp_providers();
for provider in &providers {
    println!("Found: {} (id: {})", provider.label, provider.id);
}

// Use a specific agent
if let Some(agent) = find_acp_agent_by_id("goose") {
    println!("Using Goose at: {:?}", agent.path());
}
```

## API Reference

### Discovery Functions

- `find_acp_agent() -> Option<AcpAgent>` - Find any available agent (prefers Goose)
- `find_acp_agent_by_id(id: &str) -> Option<AcpAgent>` - Find a specific agent by ID
- `discover_acp_providers() -> Vec<AcpProviderInfo>` - List all available agents

### Prompt Functions

- `run_acp_prompt(agent, working_dir, prompt) -> anyhow::Result<String>` - Send a one-shot prompt

### Types

- `AcpAgent` - Enum representing an agent (Goose, Claude, Codex)
- `AcpProviderInfo` - Information about an available provider (id, label)

## Supported Agents

| Agent | Command | Install |
|-------|---------|---------|
| Goose | `goose` | `pip install goose-ai` |
| Claude Code | `claude-agent-acp` | Install via Anthropic |
| Codex | `codex-acp` | Install via Codex |

## How It Works

1. **Discovery**: Searches for agent executables using:
   - Login shell `which` command (to get user's PATH)
   - Direct command execution
   - Common installation paths (`/opt/homebrew/bin`, `/usr/local/bin`, etc.)

2. **Execution**:
   - Spawns the agent process with ACP mode arguments
   - Creates a `ClientSideConnection` using the agent-client-protocol SDK
   - Initializes the protocol connection
   - Creates a new session
   - Sends the prompt
   - Collects text responses from `AgentMessageChunk` notifications
   - Returns the accumulated response

3. **Protocol**: Uses `agent-client-protocol` v0.9 for reliable communication

## Integration Examples

### With builderbot-actions

```rust
use acp_client::{find_acp_agent, run_acp_prompt};
use builderbot_actions::AiProvider;
use async_trait::async_trait;

pub struct AcpAiProvider {
    working_dir: PathBuf,
}

#[async_trait]
impl AiProvider for AcpAiProvider {
    async fn prompt(&self, prompt: String) -> anyhow::Result<String> {
        let agent = find_acp_agent()
            .ok_or_else(|| anyhow::anyhow!("No ACP agent found"))?;

        run_acp_prompt(&agent, &self.working_dir, &prompt).await
    }
}
```

### With Tauri

```rust
use acp_client::{find_acp_agent, run_acp_prompt};

#[tauri::command]
async fn ask_ai(question: String) -> Result<String, String> {
    let agent = find_acp_agent()
        .ok_or("No ACP agent installed")?;

    run_acp_prompt(&agent, Path::new("."), &question)
        .await
        .map_err(|e| e.to_string())
}
```

## Design Decisions

### Why Extract This?

The original codebase had working ACP code in `/Users/mtoohey/Code/staged/src-tauri/src/ai/client.rs` that reliably communicated with ACP agents. However, the action detection feature needed ACP support and had a buggy manual implementation.

Rather than duplicate or fix the manual implementation, we extracted the proven code into a reusable crate.

### Why Not Use the Full Session API?

This crate focuses on one-shot prompts because that's what action detection needs. The full session management (with streaming, events, history, etc.) remains in the main application code since it's more application-specific.

### Why agent-client-protocol v0.9?

The agent-client-protocol crate is in active development. Version 0.9 is what the working code uses, so we keep that version for compatibility and stability.

## Thread Safety

The crate uses `spawn_blocking` with a `LocalSet` to handle the ACP protocol's `!Send` futures. This means:
- All functions are safe to call from async contexts
- The agent process runs on a dedicated thread
- No blocking of the async runtime

## Error Handling

The crate uses `anyhow::Result<T>` internally for better error context and chaining. This provides:
- Clear error messages with contextual information
- Error chain tracing for debugging
- Easy conversion to `String` at API boundaries (e.g., Tauri commands) using `.map_err(|e| e.to_string())`

Common error messages:
- "No ACP agent found" - No compatible agent installed
- "Failed to spawn {agent}" - Agent process failed to start
- "Failed to initialize ACP connection" - Protocol handshake failed
- "Failed to send prompt" - Communication error

## License

Same as parent project (MIT).

## Contributing

This crate is extracted from the Builderbot project. Contributions should maintain:
1. Simple, focused API for one-shot prompts
2. Compatibility with agent-client-protocol v0.9
3. Framework-agnostic design
4. Clear error messages

## Credits

Original implementation from the Staged codebase by the Builderbot team.
