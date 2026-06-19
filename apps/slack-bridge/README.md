# slack-bridge

A minimal Slack bridge for builderbot. Mention `@builderbot` in a channel and it
starts an ACP agent task, then streams progress back into the same thread.

- **Socket Mode** — no public webhook URL required.
- **Secrets via environment** — nothing is hard-coded; see [`.env.example`](.env.example).
- **Reuses builderbot primitives** — runs agents through `crates/acp-client`
  (`AcpDriver` + `MessageWriter`), so any installed ACP agent (Codex, Claude,
  Goose, Pi, Amp) works.

## How it works

```
Slack app_mention  ──▶  events::parse_app_mention  ──▶  AgentTask
                                                          │
                                  runner::run_task (AcpDriver)
                                                          │
   StreamingWriter (impl MessageWriter)  ──▶  ProgressState ──render──▶ SlackSink
                                                          │
                          chat.postMessage (once) + chat.update (throttled)
```

The agent's streamed text and tool calls are folded into a single thread message
that is edited as work progresses (throttled to respect Slack rate limits).

## Prerequisites

- An ACP agent on your `PATH`. Default is Codex (`codex-acp`); override with
  `BUILDERBOT_AGENT`. Others: `claude` (`claude-agent-acp`), `goose`, `pi`, `amp`.
- Rust toolchain via the repo's hermit environment (`source ./bin/activate-hermit`).

## Slack app setup

1. **Create an app** at <https://api.slack.com/apps> → *From scratch*.
2. **Socket Mode**: *Settings → Socket Mode* → enable. When prompted, generate an
   **App-Level Token** with the `connections:write` scope. This is your
   `SLACK_APP_TOKEN` (`xapp-…`).
3. **Bot token scopes**: *Features → OAuth & Permissions → Scopes → Bot Token
   Scopes*, add:
   - `app_mentions:read` — receive `@builderbot` mentions
   - `chat:write` — post and edit thread messages
   - `channels:history` — read message context (add `groups:history`,
     `im:history`, `mpim:history` if you use the bot in those surfaces)
4. **Event subscriptions**: *Features → Event Subscriptions* → enable. Under
   *Subscribe to bot events*, add `app_mention`. (No Request URL is needed in
   Socket Mode.)
5. **Install**: *Settings → Install App* → install to your workspace. Copy the
   **Bot User OAuth Token** (`xoxb-…`) into `SLACK_BOT_TOKEN`.
6. **Invite** the bot to a channel: `/invite @builderbot`.

> `SLACK_SIGNING_SECRET` is **not** required for Socket Mode (it verifies HTTP
> request signatures). It is accepted for forward-compatibility only.

## Configuration

Copy `.env.example` to `.env` and fill in the tokens, or export the variables in
your shell:

| Variable | Required | Purpose |
| --- | --- | --- |
| `SLACK_APP_TOKEN` | yes | Socket Mode connection (`xapp-…`) |
| `SLACK_BOT_TOKEN` | yes | Web API calls (`xoxb-…`) |
| `SLACK_SIGNING_SECRET` | no | Reserved; unused in Socket Mode |
| `BUILDERBOT_AGENT` | no | ACP provider id, default `codex` |
| `BUILDERBOT_WORKDIR` | no | Agent working dir, default current dir |

This binary reads variables already present in the environment. To load a `.env`
file, source it first (e.g. `set -a; source apps/slack-bridge/.env; set +a`).

## Running

```bash
source ./bin/activate-hermit

# Offline check — no tokens, no network. Runs a captured event through the
# parser + progress formatter and prints what would be posted.
cargo run -p slack-bridge -- dev apps/slack-bridge/fixtures/app_mention.json

# Live dry-run — connects to Slack, echoes the parsed prompt and a canned
# progress sequence into the thread, but does NOT invoke an agent.
cargo run -p slack-bridge -- run --dry-run

# Live — mentions start real agent tasks.
cargo run -p slack-bridge -- run
```

## Tests

```bash
cargo test -p slack-bridge
```

Focused unit tests cover:
- **Slack event parsing** (`src/events.rs`) — mention stripping, thread vs.
  root replies, multiline prompts, and ignored events (bot-authored,
  non-mention, empty, missing fields).
- **Progress formatting** (`src/progress.rs`) — tool-call lines, headers,
  truncation to Slack limits, char-boundary safety, and chunk coalescing.
- **Streaming writer** (`src/runner.rs`) — tool calls flush immediately, text
  chunks are throttled, and `finalize` forces a flush.

## Dependencies

Socket Mode is implemented directly over `tokio-tungstenite` (WebSocket) plus a
few `reqwest` Web API calls, rather than a full Slack framework — the envelope
loop is small and this keeps the dependency tree minimal. Agent execution reuses
`crates/acp-client`.
