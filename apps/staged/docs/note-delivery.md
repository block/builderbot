# Note delivery through `write_note`

A note session (a branch note, a project note, or a repo-note child of a
project session) delivers its note by calling the `write_note` MCP tool. The
call is the deliverable: Staged stores exactly what the agent passes and does
not read the agent's assistant text. This replaced the earlier contract where
the note was the text after a `---` separator in the final assistant message.

## Tool contract

Staged offers a required MCP server named `staged` to every note session. It
exposes one tool:

```text
write_note(title: string, body: string, suggested_next_steps?: object | null)
```

- `title` — plain text, shown as the note heading. A leading `#` is stripped.
- `body` — the full note in markdown, without the title.
- `suggested_next_steps` — `{ "suggestedNextCommitStep": string | null,
"suggestedNextNoteStep": string | null }`, the same shape the old
  `suggested-next-steps` JSON block carried. Omitting the argument keeps the
  steps already stored on the note (a rewriting turn without the block used to
  behave the same way).

Each call replaces the whole note; the last call of the turn wins. The result
text is `Saved note "<title>" (<n> characters). The note is stored; end your
turn now without repeating it in your reply.` An empty or multi-line title or
an empty body is rejected with `isError: true` and a message asking the agent
to call again. The message writer treats a completed `write_note` call like
the old separator: the session finishes after the response that follows it
instead of holding for background work.

The prompt builders (`build_full_prompt`, the project-session action
instructions, the note follow-up prompt, and the Pikchr correction prompt) all
use `session_commands::note_delivery_guidance`, which tells the agent to call
the tool and that its assistant text is not saved.

Source: `src-tauri/src/note_tool.rs` (schema, handler, in-process server).

## Local and project sessions

Locally the server is an in-process rmcp streamable-HTTP server bound to
`127.0.0.1:0`, exactly like the `project` and `pikchr` servers, and is passed
to `session/new` as `McpServer::Http`. The handler writes into a per-run
`NoteSubmissionSlot` shared with the message writer, the post-turn Pikchr
validation (which checks the submitted body, since the transcript no longer
carries the note), and the post-completion hooks.

## Remote (Blox) sessions and the workstation outbox

A remote agent cannot reach the laptop's loopback servers, and the laptop may
disconnect before the agent finishes. Remote note sessions therefore get the
same tool from a dependency-free stdio MCP helper that lives on the
workstation (`src-tauri/resources/remote/staged-note-mcp.cjs`, Node standard
library only). `node` was verified on a Blox workstation (`sq blox ws exec
<ws> -- node --version`, v26); `python3` is present too but unused.

At remote session start, `remote_note_outbox::install_helper`:

1. runs one `ws exec sh -c` probe that creates `~/.staged/bin`, and prints the
   home directory, the recorded checksum and the path of `node`;
2. uploads the embedded helper through the existing base64 chunked
   `write_bytes_to_remote_with` path only when the recorded SHA-256 in
   `~/.staged/bin/staged-note-mcp.sha256` differs from the embedded source,
   then records the new checksum;
3. clears this session's outbox (the laptop is connected at this point);
4. injects `McpServer::Stdio { command: <node>, args: [<helper>, <session id>,
<turn id>] }` into `session/new` / `session/load`.

The helper speaks newline-delimited JSON-RPC (`initialize`, `ping`,
`tools/list`, `tools/call`, and ignores notifications). Each `write_note` call
writes atomically (temp file + rename) into the outbox:

```text
~/.staged/sessions/<session-id>/note.md    body (markdown, no title)
~/.staged/sessions/<session-id>/meta.json  {"title","suggestedNextSteps","turn","writtenAt","helperVersion"}
~/.staged/sessions/<session-id>/done       marker, written last
```

Session ids are validated to `[A-Za-z0-9_-]` before they are interpolated into
remote paths. The `turn` value is a per-run id the helper was started with, so
the collector can tell this run's note from an earlier one.

### Collector

After the turn settles, if the slot is empty and the session was not stopped,
`remote_note_outbox::collect` runs one `ws exec sh -c` that prints
`done`/`pending` plus `meta.json` and `note.md` as base64 lines (base64 keeps
arbitrary markdown out of the line protocol). A `Delivered` result becomes a
`NoteSubmission` with source `Outbox` and flows into the unchanged
`resolve_note_title_and_body` and `ingest_note_media`. A `done` marker from a
different turn is ignored (`StaleTurn`), and a missing marker is `Pending`.

If the connection to the remote agent fails but the outbox holds this run's
completed note, the turn is recorded as `completed` / `turn_complete` and the
hooks run from the outbox. A Pikchr repair loop that gave up is excluded from
that rescue because its note is known bad.

### Permissions never block on the laptop

Remote agents run unattended by construction. `AcpDriver::for_workspace`
passes `--env-vars=GOOSE_MODE=auto` to `sq blox acp` for Goose, and for the
Claude bridge sends `session/set_mode` to `bypassPermissions` right after
`session/new` / `session/load` when the agent lists that mode (best-effort,
logged otherwise). The `initialize` request advertises no `fs/*` or
`terminal/*` client capabilities, so a remote agent never routes file or
terminal access through the laptop.

## Fallback: no `write_note` call

If a note session's turn ends with no `write_note` call (and no `---`
separator text, see below), the session runner sends one follow-up prompt
(`NOTE_WRITE_FOLLOWUP_PROMPT`) asking the agent to call the tool now. It does
this once; a second silent turn stores the empty note as before, so a session
cannot loop.

The `---` parser is kept only as a fallback. When the hooks receive no
submission they scan the turn's assistant messages as before, and log at warn
level (`no write_note call this turn; falling back to the "---" separator
parser`) when that fallback produces a note.

## Detached recovery

`recover_dead_sessions` (startup and the 60 s sweep) used to mark every
running session whose owner process died as `error` / `app_quit` and skip the
hooks. For a note session on a branch with a Blox workspace it now spawns a
collection thread instead (guarded against overlapping sweeps):

- `done` marker present → hooks run with the outbox note, row becomes
  `completed` / `turn_complete`.
- otherwise → row becomes `error` / `detached` with a message naming the
  workstation. `detached` is resumable; the agent may still be working there.

`CompletionReason::Detached` is a new enum variant (backend `store/models.rs`,
frontend `src/lib/types.ts`, `RESUMABLE_REASONS`, failure copy). It is a data
model addition and is flagged for human review per `AGENTS.md`.

## How to test manually

Local:

1. Start a note session on a local branch (or a project note). In the chat
   pane, the agent's tool-call row shows `write_note` with the title/body
   arguments collapsed; the note row updates when the turn ends.
2. Check the log for `note delivered via write_note (Tool)`. If you see the
   warn line about the `---` fallback, the agent ignored the tool: check the
   prompt reached it (`note_delivery_guidance` text) and that the `staged`
   server appears in the ACP `session/new` request.
3. Put an invalid Pikchr block in the body: the correction prompt must ask the
   agent to call `write_note` again, and the stored note must be the repaired
   one.
4. Ask the agent (in the prompt) to not call any tools: after the turn, one
   follow-up user message asking for `write_note` must appear, and the note
   must be stored after the second turn.

Blox:

1. Confirm the workstation has `node`: `sq blox ws exec <ws> -- node --version`.
2. Start a note session on a remote branch. The log shows
   `remote write_note helper attached (turn <id>)`; on the workstation,
   `~/.staged/bin/staged-note-mcp.cjs` and `.sha256` exist, and after the call
   `~/.staged/sessions/<session-id>/` holds `note.md`, `meta.json`, `done`.
   A second session logs the helper as current and uploads nothing.
3. Disconnect test: start a remote note session with a long prompt, then quit
   Staged (or kill the `sq blox acp` child). Relaunch Staged: the startup
   sweep logs `Recovered detached remote session … note "<title>" found` and
   the note row fills in; if the agent had not finished, the session shows
   "Staged lost this remote session before it delivered its result." and
   offers Resume. Kill the agent before it writes, and the row must read
   `detached`.
4. Drive the helper by hand on the workstation:
   `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | node
~/.staged/bin/staged-note-mcp.cjs test-session t1`.

Automated coverage: `note_tool::tests` (schema, handler, last-call-wins),
`remote_note_outbox::tests` (install sequence, outbox parsing, helper run as a
subprocess with fake JSON-RPC, local `sh` run of the install and collect
scripts, schema parity with the Rust tool), `acp-client` driver tests (stdio
injection and unattended mode for the `goose` and `claude` providers,
`initialize` capabilities), `blox-cli` (`--env-vars`), and the session runner
and prompt tests for the fallback and follow-up paths.
