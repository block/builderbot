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
the tool and that its assistant text is not saved. The guidance names the
tool both as `write_note` and by its Claude Code name `mcp__staged__write_note`,
and says it is available now: the Claude bridge defers MCP tools behind
`ToolSearch`, and an agent that only knows the short name spends a search
round trip (or gives up) before it can call it.

In the chat pane a `write_note` call (`mcp__staged__write_note` from Claude,
`staged: write note` from Goose) is not shown as a generic "Ran" row: it gets
its own "Saved note" / "Saving note" / "Note rejected" row named after the
title, with the body as a collapsed preview and the suggested next steps
(`noteToolCall.ts`, `NoteToolDetails.svelte`). Tool calls still `pending` or
`in_progress` when the session has settled (interrupted, crashed, detached)
render as "Interrupted" instead of spinning; the backend does not rewrite
their status, the transcript builder derives it from the session status.

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
3. settles this session's outbox (`recover_uncollected_outbox`): a `done`
   marker already waiting there is a note the agent finished that no run
   ingested (the previous run lost the workstation, the sweep could not reach
   it either, the row went `detached`, and this is the resume). It is handed
   to the post-completion hooks — bound to this session's note — and only
   then is the outbox emptied for the new run. The outbox is never cleared
   blind at start. The read's failures are told apart
   (`CollectError::Transport` vs `CollectError::Content`): an unreachable
   workstation leaves the outbox alone and is logged, while unusable content
   (`meta.json` that does not parse, a body that is not valid base64, a title
   the tool would reject) is logged, cleared and skipped, because nothing
   will ever make it ingestible. Either way the helper is still attached and
   the run starts: a recovery failure must not cost the session its tool;
4. injects `McpServer::Stdio { command: <node>, args: [<helper>, <session id>,
<turn id>] }` into `session/new` / `session/load`.

Steps 1 and 3 are two `ws exec` round trips before the agent starts (a
third and fourth when the helper has to be uploaded or an uncollected note is
waiting), each a few seconds on a cold `sq blox` connection. That is the
price of a session whose deliverable survives the laptop disconnecting.

The helper speaks newline-delimited JSON-RPC (`initialize`, `ping`,
`tools/list`, `tools/call`, and ignores notifications). Each `write_note` call
writes atomically (temp file + rename) into the outbox:

```text
~/.staged/sessions/<session-id>/note.md    body (markdown, no title)
~/.staged/sessions/<session-id>/meta.json  {"title","suggestedNextSteps","turn","writtenAt","helperVersion"}
~/.staged/sessions/<session-id>/done       marker, removed first and written last
```

A rewrite (a second `write_note` call in the same turn) unlinks `done` before
it touches `note.md`, and recreates it only after both files are renamed into
place, so a reader that sees the marker — even the sweep, which can land at
any moment — always gets a title and body from the same call. `initialize`
answers with the client's MCP protocol version when the helper implements it
(`2025-06-18`, `2025-03-26`, `2024-11-05`), otherwise with the newest one it
speaks; it never echoes a version it does not implement.

Session ids are validated to `[A-Za-z0-9_-]` before they are interpolated into
remote paths. The `turn` value is a per-run id the helper was started with, so
the collector can tell this run's note from an earlier one.

### Collector

When a turn settles cleanly, `remote_note_outbox::collect_turn_into_slot` runs
one `ws exec sh -c` that prints `done`/`pending`, `meta.json` and `note.md` as
base64 lines (base64 keeps arbitrary markdown out of the line protocol), and
then `done`/`pending` again. The marker is re-tested after the files were
read because the helper unlinks it while a rewrite is in flight: if the two
readings disagree the files may be from different calls, and the snapshot is
treated as `Pending` and read again later rather than trusted. A
`Delivered` result for this run's turn id becomes a `NoteSubmission` with
source `Outbox` and is stored in the same `NoteSubmissionSlot` the local tool
fills. From there it feeds, with no further round trip:

- the post-turn Pikchr validation (`validate_submitted_note_pikchr`), so a
  remote note with a broken diagram gets the same correction prompt a local
  one does; the repair turn's rewrite is collected again when that turn
  settles, and the loop gives up after the usual number of attempts;
- the "did this turn deliver a note" decision behind the `write_note`
  follow-up prompt, so a remote turn that did call the tool is never asked
  to call it again — the follow-up fires only when the outbox has no `done`
  for this turn _and_ the `---` fallback found nothing;
- the post-completion hooks (`resolve_note_title_and_body`,
  `ingest_note_media`), unchanged.

A `done` marker from a different turn is ignored (`StaleTurn`), and a missing
marker is `Pending`. A turn that ends in a connection error never reaches the
in-loop collection; the runner then collects once more after the run
(`lost_remote_agent_terminal_state`):

- this run's completed note is there and its body passes the Pikchr check →
  `completed` / `turn_complete`, hooks run from the outbox;
- the note is there but a Pikchr block does not parse → `error` / `detached`
  with a message naming the block, and the note is _not_ stored: the
  correction loop that would have repaired it was lost with the connection,
  and storing a known-bad diagram silently is worse than asking for a Resume;
- no note → `error` / `detached` with the "keeps checking" message, and the
  sweep polls the outbox from then on (see _Polling a detached row_ below).

A Pikchr repair loop that gave up (`AgentRunError::PikchrRepairExhausted`,
matched as a variant, not by message text) never enters that rescue because
its note is known bad.

The outbox is cleared at the end of every run that observed its state: a
clean turn (note ingested or not), a Stop after a `write_note` call, an
exhausted repair loop, a crash, and the lost-connection cases above except
the last. The clear is one more `ws exec` after the terminal status is
written, so it never holds the row in `running`, and it removes the now-empty
`~/.staged/sessions/<session-id>/` directory too. Only a lost connection
with _no_ note found leaves the outbox in place, because the agent may still
write into it and the sweep (or the next run's start-of-run recovery) is
what reads it. The outbox is never cleared at start, which is what lets that
recovery tell an uncollected note from an old one.

### Background hold ends late for remote note sessions

Locally, a `write_note` call fills the slot while the turn is still running,
and the message writer's `should_finish_after_response` ends a background
hold as soon as the response after it completes. Remote sessions have no such
mid-turn signal: the outbox is only read after the turn settles, and the
prompt tells the agent not to write `---` text, so `should_finish_after_response`
never fires for them. A remote note session under a background hold therefore
runs to the hold's quiescence timeout (or its cap) even when the note was
saved early. Polling the outbox during the turn would cost a `ws exec` per
poll; this is a known limitation, not an oversight.

### Permissions never block on the laptop

Remote agents run unattended by construction, and this applies to every
remote session type — commit sessions, pipeline hand-offs and project
sessions as well as note sessions — by design: a permission prompt routed to
a laptop that may have disconnected would hang the agent on the workstation.
`AcpDriver::for_workspace` passes `--env-vars=GOOSE_MODE=auto` to `sq blox
acp` for Goose, and for the Claude bridge sends `session/set_mode` to
`bypassPermissions` right after `session/new` / `session/load` when the agent
lists that mode (best-effort, logged otherwise). The `initialize` request
advertises no `fs/*` or `terminal/*` client capabilities, so a remote agent
never routes file or terminal access through the laptop.

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
collection thread instead (guarded against overlapping sweeps in-process):

1. The row is claimed first: `transition_from_running` to `error` /
   `detached`. Two Staged instances sharing `~/.staged/data.db` both see the
   dead `owner_pid`, and only the one that wins this write goes on; the other
   returns without a single `ws exec`, so the outbox is collected and the
   hooks run exactly once.
2. The claimant collects the outbox (any turn id). `done` marker present →
   hooks run with the outbox note, the outbox is cleared, and the row is
   moved to `completed` / `turn_complete` through
   `Store::finish_detached_recovery`, which only touches a row still in the
   claimed state. Otherwise (no marker, or the workstation could not be
   read) the row stays `error` / `detached` with a message naming the
   workstation that ends in the "keeps checking" marker sentence, and the
   sweep goes on polling it. `detached` is resumable; the agent may still be
   working there.
3. If the user resumed the session while the workstation was being read, the
   final write finds the row `running` again and leaves it to its new run,
   which will find the note at start (see step 3 of the remote start sequence).

`CompletionReason::Detached` is a new enum variant (backend `store/models.rs`,
frontend `src/lib/types.ts`, `RESUMABLE_REASONS`, failure copy). It is a data
model addition and is flagged for human review per `AGENTS.md`.

### Polling a detached row

Losing the laptop does not stop the agent. Blox keeps the ACP process and its
current turn alive when the client side of `sq blox acp` goes away (the
orchestrator holds the turn; the next `sq blox acp` for the same workspace
reuses the process rather than starting a new one), so a `detached` row is
usually a note that is still being written, not a lost one. Marking it
`crashed` and waiting for a manual Resume would hide a note that arrives a
minute later. The 60 s sweep therefore keeps reading the outbox of every
`error` / `detached` row for a bounded window (`poll_detached_remote_note_sessions`):

- **Which rows.** Only rows whose `error_message` ends with
  `AWAITING_REMOTE_NOTE_SUFFIX` ("the workstation agent may still be running,
  so Staged keeps checking for its note.") are polled. That marker is how a
  row says "still waiting" without a schema change; the frontend mirrors it
  as `REMOTE_NOTE_POLL_SUFFIX` in `sessionModalHelpers.ts` and a test keeps
  the two strings equal. A detached row without it (an expired window, a
  known-bad note, a row written before polling existed) is never polled.
- **For how long.** 30 minutes (`DETACHED_NOTE_POLL_WINDOW`) from the row's
  `updated_at`, i.e. from the detaching write. Polls that find nothing write
  nothing to the row, so the window keeps counting from the detach; the
  first poll after it ran out replaces the message with
  `detached_note_expired_message` ("…no completed note appeared on the
  workstation within 30 minutes. Resume to check again and continue.") through
  the guarded `finish_detached_recovery`, so a row that was resumed or
  completed by another instance in the meantime is untouched. The row stays
  `detached`, resumable, and Resume still finds a late note through the
  start-of-run recovery.
- **Claim first.** The claim _is_ the terminal write: a poll that reads
  `done` calls `finish_detached_recovery` to `completed` / `turn_complete`
  before it runs the hooks. Two instances sharing the database can both read
  the marker, but only the one whose write lands ingests the note and clears
  the outbox; the other, and a poll racing a Resume, return without touching
  anything.
- **How many at once.** Each poll is a `ws exec` round trip on its own
  thread, at most `DETACHED_NOTE_POLL_MAX_CONCURRENT` (2) in flight across
  polls and startup collections, served oldest-unpolled first across ticks,
  so an instance that died holding several remote note sessions does not
  fan out a burst of `sq blox` calls.

What the user sees: the timeline row says "Lost the remote session — no note
created yet"; the chat pane says "Staged lost the connection to this remote
session. The workstation agent may still be running; Staged is checking for
its note and will complete the session when it appears." while the marker is
present, and the generic lost-session copy with Resume once it is not. The
session completes on its own, through the normal hooks, when the note
appears.

## How to test manually

Local:

1. Start a note session on a local branch (or a project note). In the chat
   pane, the agent's tool call shows as a "Saved note" row named after the
   title, with the body as a collapsed preview; the note row updates when the
   turn ends.
2. Check the log for `note delivered via write_note (Tool)`. If you see the
   warn line about the `---` fallback, the agent ignored the tool: check the
   prompt reached it (`note_delivery_guidance` text) and that the `staged`
   server appears in the ACP `session/new` request.
3. Put an invalid Pikchr block in the body: the correction prompt must ask the
   agent to call `write_note` again, and the stored note must be the repaired
   one. (Applies to remote sessions too: the outbox is collected after every
   settled turn, so the same correction prompt and re-collection happen there.)
4. Ask the agent (in the prompt) to not call any tools: after the turn, one
   follow-up user message asking for `write_note` must appear, and the note
   must be stored after the second turn.

Blox:

1. Confirm the workstation has `node`: `sq blox ws exec <ws> -- node --version`.
2. Start a note session on a remote branch. The log shows
   `remote write_note helper attached (turn <id>)`; on the workstation,
   `~/.staged/bin/staged-note-mcp.cjs` and `.sha256` exist, and after the call
   `~/.staged/sessions/<session-id>/` holds `note.md`, `meta.json`, `done`.
   A second session logs no `Installed write_note helper` line and uploads
   nothing (the probe's checksum matches).
3. Disconnect test: start a remote note session with a long prompt, then quit
   Staged (or kill the `sq blox acp` child). Relaunch Staged: the startup
   sweep logs `Recovered detached remote session … note "<title>" found` and
   the note row fills in; if the agent had not finished, the chat pane shows
   "Staged lost the connection to this remote session. The workstation agent
   may still be running; Staged is checking for its note…" and the row offers
   Resume. Leave it: when the agent finishes on the workstation, a later
   sweep logs `note "<title>" appeared in the workstation outbox; completing
the session` and the note row fills in with no click. Kill the agent
   before it writes (`pkill` the agent on the workstation, not `sq blox acp`),
   and the row must stay `detached`; after 30 minutes its message changes to
   the "no completed note appeared … within 30 minutes" copy. Write the note
   by hand (step 5) and press Resume: the new run's log shows `uncollected
note … from an earlier run; ingesting it before this run starts`, the
   note row fills in before the new turn, and `~/.staged/sessions/<session-id>/`
   is gone afterwards.
4. Run two Staged builds against the same data dir and repeat step 3: only
   one of them logs `note "<title>" found in the workstation outbox` (or
   `appeared in the workstation outbox`); the other logs nothing for the
   session.
5. Drive the helper by hand on the workstation:
   `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | node
~/.staged/bin/staged-note-mcp.cjs test-session t1`.

Automated coverage: `note_tool::tests` (schema, handler, last-call-wins),
`remote_note_outbox::tests` (install sequence, outbox parsing including a
marker that changed between the reads, transport vs content failures,
per-turn collection into the slot, start-of-run recovery that discards
unusable content, helper run as a subprocess with fake JSON-RPC including a
rewrite blocked between its renames and protocol-version negotiation, local
`sh` run of the install, collect, clear and recovery scripts, schema parity
with the Rust tool), `session_runner::tests::detached_recovery` (claim-first
sweep: one winner, resume during collection; the poll window and its marker;
a poll that finds the note claims then ingests once; polls that find nothing
write nothing; expiry is guarded and ends the polling; the lost-connection
terminal states; the end-of-run clear rule; frontend marker parity),
`acp-client` driver tests (stdio injection and unattended mode for the
`goose` and `claude` providers, `initialize` capabilities), `blox-cli`
(`--env-vars`), the session runner and prompt tests for the fallback and
follow-up paths, and on the frontend `toolCallViewModel.test.ts` /
`acpTranscript.test.ts` (the `write_note` row for both agents, interrupted
pending calls), `sessionModalHelpers.test.ts` and `sessionFailureCopy.test.ts`
(the detached copy).
