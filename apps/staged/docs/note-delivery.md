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
says that Claude Code may have deferred it, and tells the agent to load it
with `ToolSearch select:mcp__staged__write_note` before calling it rather than
calling it blind. The deferral is Claude Code's, not Staged's: claiming the
tool was "available now" did not stop the bridge from deferring it (7 of 8
sessions in the retest searched anyway), and one session called it
schema-less with empty input, which `write_note` rejected. The search round
trip is accepted; the rejected empty call is what the wording avoids. The
Goose wording is unchanged.

In the chat pane a `write_note` call (`mcp__staged__write_note` from Claude,
`staged: write note` from Goose) is not shown as a generic "Ran" row: it gets
its own "Saved note" / "Saving note" / "Note rejected" row named after the
title, with the body as a collapsed preview and the suggested next steps
(`noteToolCall.ts`, `NoteToolDetails.svelte`). Tool calls still `pending` or
`in_progress` when the session has settled render by the session's outcome
(`AcpTranscriptOptions`): in a session that ended `error` or `cancelled`
they were cut off and read "Interrupted" in the collapsed header as well as
in the status; in a session that ended `completed` — a remote note session
completed from the workstation outbox — their work stood, so the `write_note`
row reads "Saved note" and other calls "Finished with the session". The
backend does not rewrite their status; the transcript builder derives it
from the session status.

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
same tool from the Staged MCP on Blox, Staged's stdio MCP server on the
workstation. How it is installed, injected and spoken to is described in
[`staged-mcp-on-blox.md`](./staged-mcp-on-blox.md). This section covers what
`write_note` does there and how the note gets back to the laptop.

At remote note session start, the session runner:

1. installs the Staged MCP on Blox (`remote_staged_mcp::install`: one probe,
   plus an upload when the script changed);
2. settles this session's outbox
   (`remote_staged_mcp::note_outbox::recover_uncollected_outbox`): a `done`
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
   will ever make it ingestible. Either way the server is still attached and
   the run starts: a recovery failure must not cost the session its tool;
3. injects the server into `session/new` / `session/load` with this run's
   turn id.

Steps 1 and 2 are two `ws exec` round trips before the agent starts (a
third and fourth when the script has to be uploaded or an uncollected note is
waiting), each a few seconds on a cold `sq blox` connection. That is the
price of a session whose deliverable survives the laptop disconnecting.

Each `write_note` call writes atomically (temp file + rename) into the
outbox, the note's three files in the session's scratch directory:

```text
~/.staged/sessions/<session-id>/note.md    body (markdown, no title)
~/.staged/sessions/<session-id>/meta.json  {"title","suggestedNextSteps","turn","writtenAt","helperVersion"}
~/.staged/sessions/<session-id>/done       marker, removed first and written last
```

A rewrite (a second `write_note` call in the same turn) unlinks `done` before
it touches `note.md`, and recreates it only after both files are renamed into
place, so a reader that sees the marker — even the sweep, which can land at
any moment — always gets a title and body from the same call.

The `turn` value is the per-run id the server was started with, so the
collector can tell this run's note from an earlier one. `helperVersion` is
the script's `SERVER_VERSION`, under the key earlier builds wrote.

### Collector

When a turn settles cleanly, `note_outbox::collect_turn_into_slot` runs
one `ws exec sh -c` that prints `done`/`pending`, `meta.json` and `note.md` as
base64 lines (base64 keeps arbitrary markdown out of the line protocol), and
then `done`/`pending` again. The marker is re-tested after the files were
read because `write_note` unlinks it while a rewrite is in flight: if the two
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
marker is `Pending`. A turn that ends because the agent was lost never reaches
the in-loop collection; the runner then collects once more after the run and
lets the outbox decide (`lost_remote_agent_detail`,
`lost_remote_agent_terminal_state`):

- this run's completed note is there and its body passes the Pikchr check →
  `completed` / `turn_complete`, hooks run from the outbox;
- the note is there but a Pikchr block does not parse → `error` / `detached`
  with a message naming the block, and the note is _not_ stored: the
  correction loop that would have repaired it was lost with the connection,
  and storing a known-bad diagram silently is worse than asking for a Resume;
- no note → `error` / `detached` with the "keeps checking" message, and the
  sweep polls the outbox from then on (see _Polling a detached row_ below).

Three endings count as losing the agent, each leaving it plausibly still at
work on the workstation with the outbox as the only way to its note:

- **A transport loss** (`remote_agent_connection_was_lost`): the connection
  settled `Failed` while serving the turn, _and_ the error is the pipe going
  away (`acp_client::is_transport_loss_error`: "transport closed",
  "connection closed", "connection reset", "broken pipe", "unexpected EOF"
  and the like). The list deliberately does _not_ include "process exited"
  or "child exited": bridges relay their own agent subprocess dying over a
  live transport in exactly those words (claude-agent-acp: "Claude Code
  process exited with code 1"; goose: "subprocess exited with status …",
  "Child exited gracefully"), and in that case the agent is gone, not the
  connection to it — nothing is working on the workstation and nothing will
  land in the outbox.
- **A Stop the agent never acknowledged**
  (`SessionSettleReason::CancelUnacknowledged`): see _A hung proxy_ below.
- **The inactivity watchdog** found this run's finished note in the outbox
  behind a silent connection and ended the turn itself: see _A hung proxy_.

Every other failure of a remote note session is an ordinary `crashed` row:
an agent that answered (`auth_required`, a refused prompt) settles `Failed`
too but through a reply; the 90 s ACP startup timeout and a `session/new`
refusal never settle at all; a local persistence error comes after a clean
settle. None of those leave an agent working on the workstation, so none
deserve 30 minutes of `ws exec` polls. Both the detached row's message and a
crashed row's carry the innermost human sentence of the error
(`innermost_error_sentence` / `innermost_error_detail`: "Incoming transport
closed"), not the Rust Debug dump of the nested protocol error; the full
error stays in the log.

A Pikchr repair loop that gave up (`AgentRunError::PikchrRepairExhausted`,
matched as a variant, not by message text) never enters that rescue because
its note is known bad.

The outbox is cleared at the end of every run that observed its state: a
clean turn (note ingested or not), a Stop the agent acknowledged after a
`write_note` call, an exhausted repair loop, a crash, and the lost-agent
cases above except the last. The clear is one more `ws exec` after the terminal status is
written, so it never holds the row in `running`, and it removes the now-empty
`~/.staged/sessions/<session-id>/` directory too. Two states keep the outbox
(`remote_outbox_is_cleared_at_run_end`, `RemoteOutboxUse`):

- a lost connection with _no_ note found, because the agent may still write
  into it and the sweep (or the next run's start-of-run recovery) is what
  reads it;
- a run whose start-of-run recovery did not settle — the workstation dropped
  between installing the Staged MCP on Blox and reading the outbox, or a
  waiting note could not be stored — and that never collected its own note. An
  earlier run's finished note may still be sitting there unread (the per-turn
  collector reports it as a stale turn and ignores it), and the clear would
  delete the only copy; the next run's recovery gets another chance. Once the
  run collects its own note the earlier one has been overwritten, and the
  clear is back on.

The outbox is never cleared at start, which is what lets that recovery tell
an uncollected note from an old one.

### A hung proxy: Stop and the inactivity watchdog

A network drop does not always close the transport. The `sq blox acp` proxy
can stay alive and blocked on a socket that will never answer; from this
side nothing fails, no error reaches the session loop, and the row sits in
`running` while the agent on the workstation finishes and writes its note.
Two mechanisms cover that:

- **Stop has a deadline.** A user Stop sends `session/cancel` and waits for
  the agent to answer the in-flight `session/prompt`. A bridge that honours
  the cancel answers within a few seconds; a hung proxy never does. After
  `acp_client::REMOTE_CANCEL_ACK_TIMEOUT` (10 s; `CANCEL_ACK_TIMEOUT` is the
  local agent's, the same figure) the driver settles the turn `Cancelled`
  with reason `CancelUnacknowledged` instead of a plain `Cancelled`, and for
  a remote note session the runner reads that as a lost agent: it collects
  the outbox once more, and the row ends `completed` from this run's note if
  it is there, otherwise `error` / `detached` with the "keeps checking"
  marker and the detail "the agent did not acknowledge the stop within 10
  seconds; the stop may still have reached it, so a note may or may not
  appear". The copy is deliberately uncommitted: the cancel was written to
  a proxy that answered nothing, and whether it got through before the proxy
  was killed is unknowable from here (the chat pane says the same, keyed on
  `UNACKNOWLEDGED_STOP_DETAIL_MARKER`). The outbox is kept either way, so a
  note the agent finishes a minute later is picked up by the sweep. A Stop
  the agent _did_ acknowledge is still a plain Stop (see _Design
  decisions_). The deadline was measured on Blox: against a silent 90 s
  shell loop both the Claude and the Goose bridge acknowledged a Stop in
  0.6 s (three runs, 2026-11-01), so 10 s leaves a wide margin over the
  proxy and orchestrator hops.

  The cancel that precedes an interrupt-and-reprompt has its own, longer
  deadline (`acp_client::INTERRUPT_ACK_TIMEOUT`, 30 s): a wedged bridge gets
  no second prompt and the session settles `CancelUnacknowledged` too. For a
  remote note session that is a lost agent _only if the connection relayed
  nothing from the agent for the whole wait_ (`lost_remote_agent_detail`
  reads the writer's `ActivityClock`); an agent that kept streaming while a
  long tool call wound down was slow, not lost, and the run ends `cancelled`
  as a local one would.

  `graceful_stop` then kills the proxy. After an unacknowledged cancel the
  proxy is known to answer nothing, so its whole process group is SIGKILLed
  at once, with no SIGINT grace; otherwise the group gets SIGINT and up to
  five seconds for the `sq` wrapper to exit, and then the _group_ is
  SIGKILLed regardless — whether the wrapper exited on the SIGINT or sat
  out the grace. `sq blox acp` wraps a `blox.exoskeleton` child, and
  killing only the wrapper, or trusting the wrapper's exit, left a frozen
  child orphaned: the kernel SIGCONTs a stopped process whose group has
  lost its last running member, and the revived proxy then relayed the
  queued cancel to the workstation and ended the turn the user had been
  told might still finish. SIGKILL is delivered to a stopped process, so
  the group kill reaches the frozen child, and an already-empty group just
  answers `ESRCH` (verified on macOS by `acp-client`'s `session_isolation`
  tests with a SIGSTOPped grandchild under both a SIGINT-ignoring and a
  SIGINT-honouring wrapper). Cost of an unacknowledged Stop on a remote
  note session: the 10 s deadline plus one `ws exec` outbox read, about
  13 s to the row settling (it was 17 to 18 s while the five-second grace
  was still offered to a proxy that could not use it).

- **One connection, one session.** The Blox proxy reuses a long-lived
  workstation bridge process, which relays _every_ session's
  `session/update`, async-task and raw SDK frames down every open
  connection. The driver records the ACP session id each connection serves
  (before `session/load`, whose replay streams updates ahead of its
  response, or right after `session/new`) and drops frames stamped with
  another session id at debug level before they reach the message writer,
  the task sets or the activity signal — so an older session finishing its
  turn on the workstation is neither rendered in the wrong chat pane nor
  counted as activity by the wrong session's holding debounce or inactivity
  watchdog. The gate is closed, not open, while the session is still
  unknown: a fresh connection receives the older session's streaming turn
  from the moment it is attached, during the `initialize` and
  `session/new` round trips, and nothing legitimate can arrive for a
  session that does not exist yet, so every stamped frame before
  `session/new` answers is dropped too.
- **Silence triggers a probe.** Each connection of a remote note session with
  the Staged MCP on Blox attached runs an inactivity watchdog
  (`session_runner::remote_note_watchdog`) against the turn. The message
  writer bumps an `ActivityClock` on every event the driver hands it; once
  the agent has been silent for `DEFAULT_INACTIVITY_PROBE` (10 minutes,
  overridable with `STAGED_REMOTE_NOTE_INACTIVITY_PROBE_SECS`, whole seconds)
  the watchdog runs the per-turn collector — one cheap `ws exec` — on a
  blocking thread. A `done` for this turn pulls the note into the slot and
  ends the turn: the connection's own cancel token (a child of the session's,
  so a Stop still reaches it, but one the loop can fire without it reading as
  a Stop) is cancelled, the driver sends `session/cancel` and gives up after
  the ack deadline, and the terminal state is the lost-agent one with the
  note delivered: `completed` / `turn_complete`, hooks run from the outbox.
  Anything else — `pending`, a stale turn, an unreadable outbox — keeps
  waiting and probes again after another silent interval, so a long-running
  agent that is merely quiet costs one round trip per ten minutes and is
  never interrupted. The watchdog races the prompt only; it is dropped the
  moment the turn ends on its own.

  "A `done` for this turn" is read strictly. Every prompt turn of a run shares
  one turn id (the Staged MCP on Blox is attached once) and the outbox is
  cleared at the end of the run, not between turns, so during a Pikchr repair
  or a `write_note` follow-up turn the previous turn's `done` is still on the
  workstation. The per-turn collector remembers the note it last pulled
  (`note_outbox::CollectedNote`: the `writtenAt` stamp plus title, body and
  next steps) and the watchdog's probe (`probe_turn_into_slot`) reports an
  identical snapshot as `pending`; only a note that differs — a rewrite — ends
  the turn. Without that, a quiet repair turn would be cancelled on the very
  body the collector had already rejected, and the run would end `detached`
  with "that note was not stored". The watchdog's own completion costs the
  same 10 s cancel deadline as a Stop before the row settles.

### Background hold ends late for remote note sessions

Locally, a `write_note` call fills the slot while the turn is still running,
and the message writer's `should_finish_after_response` ends a background
hold as soon as the response after it completes. Remote sessions have no such
mid-turn signal: the outbox is only read after the turn settles, and the
prompt tells the agent not to write `---` text, so `should_finish_after_response`
never fires for them. A remote note session under a background hold therefore
runs to the hold's quiescence timeout (or its cap) even when the note was
saved early. Polling the outbox during the turn would cost a `ws exec` per
poll; this is a known limitation, not an oversight. A hold the user _stops_
before the agent has written is a different matter: the agent carries on,
and its late note is picked up by the sweep (see _Polling a hold-stopped
row_ below).

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
   hooks run with the outbox note; only when they report the note stored
   (`HooksOutcome::note_stored`) is the outbox cleared and the row moved to
   `completed` / `turn_complete` through `Store::finish_detached_recovery`,
   which only touches a row still in the claimed state. A note the hooks
   could not store (a media ingest or store write failed) leaves the row
   `error` / `detached` with a message naming the note and asking for Resume
   (`detached_note_unstored_message`), and the outbox intact, so the resume's
   start-of-run recovery collects it again. Otherwise (no marker, or the
   workstation could not be read) the row stays `error` / `detached` with a
   message naming the workstation that ends in the "keeps checking" marker
   sentence, and the sweep goes on polling it. `detached` is resumable; the
   agent may still be working there.
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

- **Which rows.** Rows whose `error_message` ends with
  `AWAITING_REMOTE_NOTE_SUFFIX` ("the workstation agent may still be running,
  so Staged keeps checking for its note.") are polled every tick. That marker
  is how a row says "still waiting" without a schema change; the frontend
  mirrors it as `REMOTE_NOTE_POLL_SUFFIX` in `sessionModalHelpers.ts` and a
  test keeps the two strings equal. Rows ending in
  `STORING_REMOTE_NOTE_SUFFIX` ("storing the note it finished on the
  workstation.", mirrored as `REMOTE_NOTE_STORING_SUFFIX`) are an instance's
  claim (below) and are left to it while the claim is fresh; once the claim
  is older than `DETACHED_NOTE_STORING_STALE` (2 minutes — storing a note is
  one `ws exec` read plus local writes) the claimant is taken to have died
  between its claim and its `completed` write, and the row is polled again
  rather than parked forever. A detached row with neither tail (an expired
  window, an unreachable workstation, a known-bad note, a row written before
  polling existed) is never polled.
- **For how long.** 30 minutes (`DETACHED_NOTE_POLL_WINDOW`) from the row's
  `updated_at`, i.e. from the detaching write (or from a stale claim, which
  restamps it). Polls that find nothing write nothing to the row, so the
  window keeps counting. A row found past the window is not expired blind:
  it gets one last poll (`DetachedNotePollAction::PollThenExpire`,
  `poll_detached_remote_note_session`), because the common case is a laptop
  closed overnight whose note has long since landed on the workstation —
  that poll completes the row on the first tick after relaunch. Only when
  that poll _reads_ the outbox and finds no note is the message replaced
  with `detached_note_expired_message` ("…no completed note appeared on the
  workstation within 30 minutes. Resume to check again and continue."). A
  poll that could not read the outbox at all (the workstation suspended,
  the network down) says nothing about the note and never expires the row
  on its own: past the window it is retried on up to
  `DETACHED_NOTE_POLL_MAX_UNREACHABLE` (5) consecutive sweeps, and only
  then is the row settled with `detached_note_unreachable_message` ("…could
  not reach the workstation afterwards to check for its note. Resume once
  the workstation is reachable…"), which says what actually happened. The
  count is per process and resets on any successful read. Either expiry is
  a guarded write (below), so a row that was resumed or completed by another
  instance in the meantime is untouched; the row stays `detached`,
  resumable, and Resume still finds a late note through the start-of-run
  recovery.
- **Claim first, complete last, every write guarded.** Each poll carries
  the row exactly as the sweep read it (`ObservedDetachedRow`: `updated_at`
  and `error_message`), and every write it makes goes through
  `Store::finish_detached_recovery_observed`, whose predicate requires the
  row to still be `error` / `detached` _with that stamp and that message_.
  A poll that reads `done` first claims the row with the storing message;
  that write restamps `updated_at`, so a second instance that read the same
  marker and the same stamp — before or after the first claim — fails its
  own claim and backs off without ingesting (the predicate on status alone
  could not tell the two apart, since the claimed row is still
  `error` / `detached`). The claimant then runs the hooks and writes
  `completed` / `turn_complete`, clearing the outbox, only when they report
  the note stored — and that final write is guarded on the stamp its claim
  returned, so a claimant that was merely slow past the staleness bound
  finds the row re-claimed by another instance and backs off too. A note
  that could not be stored leaves `detached_note_unstored_message` and the
  outbox in place for a Resume. Both sweep paths therefore fail the same
  way: toward a `detached` row with the note still on the workstation,
  never toward a `completed` row with no note, and never toward two
  instances ingesting one note.
- **How many at once.** Each poll is a `ws exec` round trip on its own
  thread, at most `DETACHED_NOTE_POLL_MAX_CONCURRENT` (2) in flight across
  polls and startup collections, served oldest-unpolled first across ticks,
  so an instance that died holding several remote note sessions does not
  fan out a burst of `sq blox` calls.

What the user sees: while the polling marker or a storing claim is present
the timeline row says "Lost the remote session — no note created yet" (the
`NoteTimelineItem` carries `isPollingForNote`, computed by the backend from
the row's completion reason and message, for that check) and the chat pane
says "Staged lost the connection to this remote session. The workstation
agent may still be running; Staged is checking for its note and will
complete the session when it appears." (or, for a storing claim, "…but the
workstation agent finished its note. Staged is storing it and will complete
the session."). Once both are gone the row drops the "yet", and the chat
pane shows the backend's own sentence (the expired window, the unreachable
workstation, the unstored note, the broken diagram) — each written to say
what Resume will do. Every `detached` row carries an error message, so the
chat pane decides the detached notice _before_ its raw-error branch
(`sessionEndNoticeKind`): the row always gets the informational alert with
its Resume button, never the destructive alert with the raw text. The
session completes on its own, through the normal hooks, when the note
appears.

### Polling a hold-stopped row

A remote note turn can complete and then sit in a background hold while the
agent works on (Claude Code backgrounding a blocked command, say). If the
user stops that wait before the agent has called `write_note`, the run ends
`completed` / `hold_stopped` and reads the outbox once on its way out — but
the Stop does not stop the workstation agent (Blox keeps the turn alive),
so the agent's `write_note` typically lands a minute _after_ the run has
stored an empty note and cleared the outbox. Nothing used to read it again
short of a Resume, and the note sat stranded in `~/.staged/sessions/<id>/`.

The sweep therefore polls these rows with the same machinery as detached
ones (`RemoteNoteParkedState::HoldStopped`), with these differences:

- **Which rows.** `Store::get_hold_stopped_sessions_awaiting_note`:
  `completed` / `hold_stopped` sessions on a branch with a Blox workspace
  (joined in SQL) whose repo note still has empty content, and that carry
  no message (no sweep has touched them yet) or a storing claim. A local
  `hold_stopped` row with an empty note is a settled end state (its agent
  stopped with the run) and is never returned. Every other message the sweep
  writes settles the row, so a row drops out of the query once its last poll
  past the window records the expiry. The query applies no clock: a row
  that went past the window while no Staged instance was running must still
  get that last poll. The poll action then makes the row due when it has no
  message, or a storing claim older than `DETACHED_NOTE_STORING_STALE`.
- **Same window, same claim.** 30 minutes from the `hold_stopped` write
  (`DETACHED_NOTE_POLL_WINDOW`), one last poll past it, at most two in
  flight, oldest-unpolled first. A poll that finds `done` first claims the
  row with `hold_stopped_note_storing_message` through
  `Store::mark_hold_stopped_note_recovery_observed`, a guarded write whose
  predicate requires the row to still be `completed` / `hold_stopped` with
  the observed stamp and message; it restamps `updated_at` and sets the
  message, leaving status and completion reason alone. Only the winner runs
  the hooks, writes the message back to nothing, and clears the outbox; the
  loser and a row the user resumed meanwhile back off. Every marker write
  additionally requires the note to still be empty: the awaiting row and
  the row whose note was just stored both carry no message, and
  `updated_at` is in milliseconds, so without that guard a claim read
  before another instance's could land on the stored row if the whole store
  took under a millisecond. A note that cannot be stored leaves
  `hold_stopped_note_unstored_message` and the outbox for a Resume. This is the one place an `error_message` is written on a
  completed row; the chat pane shows a completed row's message only for
  `error` / `cancelled` rows, so the marker is invisible there, the
  child-session status a project agent reads (`repo_session_payload`) omits
  `error_message` for a completed child, and any later status write (a
  resume) clears it.
- **The row stays `completed`.** A found note fills the note in; the row's
  status and `hold_stopped` reason do not change, the timeline never shows
  it as "polling" (`isPollingForNote` is for detached rows), and the chat
  pane keeps "The wait for this session's background work was stopped"
  with its Resume button. A status event is still emitted so the open
  views refresh the note.
- **Expiry clears the outbox.** When the last poll past the window finds no
  note, the row gets `hold_stopped_note_expired_message` (no marker tail, so
  it is never polled again) and the workstation outbox is cleared, so
  nothing is left stranded — unlike a detached row, whose expired message
  asks for a Resume that will collect the note. An unreachable workstation
  follows the detached rule (five consecutive failed reads past the
  window, then `hold_stopped_note_unreachable_message`, nothing cleared).

Rows from before this existed (any `hold_stopped` remote note session with
an empty note) are past the window on the first sweep after upgrade: each
gets one poll, a late note is stored, and the outbox is cleared either way.

## Design decisions

Decisions that were weighed and settled, so a later reader does not reopen
them by accident:

- **A Stop the agent acknowledges after `write_note` discards the note.** A
  remote note session the user stops after the agent has already called
  `write_note` ends `cancelled` with no note, the outbox is cleared at the
  end of the run, and a later Resume does not re-ingest it. This matches the
  local path (a cancelled run runs no post-completion hooks) and the rule
  that a Stop means "I do not want this turn's output". Keeping an
  already-delivered note on Stop would mean the hooks run for cancelled
  sessions, with the commit detection and media ingest that come with them;
  nothing asked for that. Revisit only if users start pressing Stop
  _because_ the note is saved and the agent is still running — the chat
  pane's "Saved note" row makes that moment visible.
- **A Stop the agent never acknowledges is a lost connection, not a Stop.**
  The user pressing Stop on a hung proxy is not a statement about the turn's
  output: it is the only button they have when the session looks stuck.
  Ending the row `cancelled` there would clear the outbox and silently
  discard a note the agent finishes a minute later on the workstation — the
  exact loss the outbox exists to prevent. So an unacknowledged cancel
  (10 s) is treated like a transport loss: `detached` with the polling
  marker, outbox kept, sweep polling. The deadline is short because a bridge
  that honours a cancel answers within a few seconds (0.6 s measured on
  Blox), and a user staring at a Stop that does not stop gains nothing from
  a longer wait. The row's copy does not promise a note, because the Stop
  may have reached the agent after all; and an interrupt's unacknowledged
  cancel counts as lost only when the agent was silent for the whole 30 s
  wait, since an interrupt is not the user giving up on the session.
- **A stopped background hold collects the outbox once, then the sweep
  takes over.** A remote note turn that completed and was then stopped
  during its background hold (`HoldStopped`) skips the in-loop collection,
  like any cancel. The agent may have called `write_note` from the
  background work the hold was waiting on, so the end of the run reads the
  outbox once more and stores a note it finds through the normal hooks (the
  row is `completed` / `hold_stopped` either way). A note the agent writes
  _after_ that read is picked up by the sweep for 30 minutes (_Polling a
  hold-stopped row_), which fills the note in without changing the row's
  status; past the window the outbox is cleared rather than left for a
  Resume, because a completed row does not ask the user to come back the
  way a detached one does.
- **The inactivity watchdog probes, it does not time out.** Ten minutes of
  silence is a reason to _look_ at the outbox, not to end the turn: an agent
  thinking through a large diff is quiet for that long legitimately. Only a
  finished note for this very turn ends it — at that point the agent has
  delivered, and the connection has demonstrably stopped relaying — and the
  result is the completed-from-outbox path, not a failure. The interval is
  an environment variable so the path can be exercised by hand without
  waiting ten minutes.
- **A failed read never expires a row by itself.** "No completed note
  appeared" is a claim about the workstation; a poll that could not reach
  the workstation cannot make it. Past the window such polls are retried a
  bounded number of times and the row is then settled with copy that says
  the workstation was unreachable, so the user knows to Resume once it is
  back rather than conclude the agent never finished.
- **Clear policy.** The outbox is cleared at the end of every run that saw
  its state, and kept only when the sweep is still polling for the note or
  when the start-of-run recovery did not settle and the run never collected
  its own note (above). The sweep clears it again after it stores a late
  note, and after a hold-stopped row's window expires. Clearing at start
  was rejected because it would delete an uncollected note on every resume;
  never clearing was rejected because a stored note would be re-ingested on
  the next run, over whatever the user edited meanwhile.
- **Detached polling window.** 30 minutes from the detaching write, with
  one last poll before expiring. Shorter windows miss the agent finishing a
  long turn (Blox keeps the turn alive); longer ones keep spending `ws exec`
  on a session that may never finish. Past the window the row is resumable
  and Resume collects a late note, so the cost of expiring is one click,
  never a lost note.
- **Only a lost agent detaches.** Auth failures, startup timeouts, refused
  prompts and local persistence errors are `crashed`, not `detached`, even on
  a remote note session with the Staged MCP on Blox attached: no agent is
  working on the workstation in those cases, so polling would be 30 minutes of
  round trips for nothing and the "may still be running" copy would be false.
  The same goes for a bridge reporting its own agent subprocess exited: that
  is the agent dying, relayed over a healthy transport, which is why "process
  exited" / "child exited" are not transport-loss phrases.
- **Remote note sessions run unattended.** Permission prompts never route
  to the laptop (see _Permissions never block on the laptop_); the session's
  deliverable has to survive the laptop going away.

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

1. Check the Staged MCP on Blox first (`node`, install, the attach log line):
   see _How to test manually_ in [`staged-mcp-on-blox.md`](./staged-mcp-on-blox.md).
2. Start a note session on a remote branch. After the `write_note` call,
   `~/.staged/sessions/<session-id>/` on the workstation holds `note.md`,
   `meta.json` and `done`.
3. Disconnect test: start a remote note session with a long prompt, then quit
   Staged (or kill the `sq blox acp` child). Relaunch Staged: the startup
   sweep logs `Recovered detached remote session … note "<title>" found` and
   the note row fills in; if the agent had not finished, the chat pane shows
   "Staged lost the connection to this remote session. The workstation agent
   may still be running; Staged is checking for its note…" in an
   informational alert with a Resume button (never a red alert with the raw
   error), and the timeline row says "no note created yet". Leave it: when
   the agent finishes on the workstation, a later sweep logs `note "<title>"
appeared in the workstation outbox; completing the session` and the note
   row fills in with no click; the chat pane's `write_note` row reads "Saved
   note", not "Interrupted". Kill the agent before it writes (`pkill` the
   agent on the workstation, not `sq blox acp`), and the row must stay
   `detached`; after 30 minutes its message changes to the "no completed
   note appeared … within 30 minutes" copy and the timeline row drops the
   "yet". Write the note by hand (step 5) into the outbox of a row that is
   already past the window: the next sweep tick must complete it (the last
   poll before expiring), with no Resume. Otherwise press Resume: the new
   run's log shows `uncollected note … from an earlier run; ingesting it
before this run starts`, the note row fills in before the new turn, and
   `~/.staged/sessions/<session-id>/` is gone afterwards.
   Auth check: sign the remote agent out and start a remote note session;
   the row must end `crashed` with the auth message, not `detached`, and the
   log must show no `Detached remote session` polling for it.
   Hung-proxy test: start a remote note session with a long prompt, find the
   proxy process (`pgrep -fl 'blox.exoskeleton acp'` — `sq blox acp` is a
   wrapper around it) and freeze it with `kill -STOP <pid>`. The transport
   stays open, so nothing happens on its own. Press Stop: the row settles
   about 13 s later (the 10 s deadline, then one outbox read; it was 17 to
   18 s before the immediate group kill), the log shows `Timed out waiting
for ACP prompt response 10s after session/cancel … the agent is
unresponsive` followed by `lost the remote agent (the agent did not
acknowledge the stop within 10 seconds; the stop may still have reached
it, so a note may or may not appear)`, and the row must be `error` /
   `detached` with the "keeps checking" message (informational alert with
   Resume whose copy says the stop may still have reached the agent,
   timeline row "no note created yet"), _not_ `cancelled`.
   `~/.staged/sessions/<session-id>/` on the workstation must still exist,
   and _both_ local proxy processes must be gone the moment the row settles
   (`pgrep -fl 'blox.exoskeleton'` finds nothing — in particular no `SN`
   continued orphan reparented to pid 1). Because the frozen proxy is
   killed, not continued, the workstation agent's loop keeps running and
   its `done` appears on schedule. When it does, the next sweep tick
   completes the row from the outbox. Run it again with the agent allowed
   to finish _before_ the Stop (`done` present in the outbox): the Stop must
   end the row `completed` with the note stored, because the unacknowledged
   stop collects the outbox first.
   Shared-bridge test: with one remote session detached and still running on
   the workstation, start a second remote session in the same workspace (the
   log shows `Found existing ACP process, reusing`) and let the first one
   finish meanwhile. Its `write_note` call and final text must appear in the
   first session's row only; the second session's log shows the frames
   `dropping a session update for another session` at debug level.
   Watchdog test: `STAGED_REMOTE_NOTE_INACTIVITY_PROBE_SECS=30`, start a
   remote note session, freeze the proxy as above and leave it. Within ~30 s
   of the last agent event the log shows `no activity from the remote agent
for 30s; checking the workstation outbox`, then every 30 s while the
   agent works; once `done` lands, `the remote agent's note "<title>" is
finished on the workstation but the connection reported nothing …
treating the connection as lost and completing from the outbox`, and the
   row ends `completed` with no click. Thaw a proxy you still have with
   `kill -CONT <pid>` to confirm a probe that finds `pending` never
   interrupts an agent that resumes talking.
   Hold test: prompt the agent to run a bare `sleep 60` (the Claude harness
   blocks it, so the agent backgrounds it and ends its turn into a hold),
   press Stop on the hold before the agent has written. The row ends
   `completed` / `hold_stopped` with an empty note and the log shows
   `workstation outbox has no completed write_note call`. About a minute
   later the workstation writes `done`; the next sweep tick logs
   `Hold-stopped remote note session <id>: note "<title>" appeared in the
workstation outbox; storing it`, the note fills in on the timeline, the
   row stays `completed` / `hold_stopped`, and `~/.staged/sessions/<id>/`
   is gone from the workstation. Expected: the chat pane keeps the
   hold-stopped copy ("The wait for this session's background work was
   stopped") and its Resume button after the sweep fills the note in; only
   the note content changes.
4. Run two Staged builds against the same data dir and repeat step 3: only
   one of them logs `note "<title>" found in the workstation outbox` (or
   `appeared in the workstation outbox`); the other logs nothing for the
   session.
5. Write a note by hand on the workstation through the Staged MCP on Blox:
   `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"Hand","body":"Written by hand"}}}' | node
~/.staged/bin/staged-mcp.cjs <session-id> <turn-id>`.

Automated coverage: `note_tool::tests` (schema, handler, last-call-wins),
`remote_staged_mcp::tests` (the Staged MCP on Blox itself: install,
injection, protocol; see [`staged-mcp-on-blox.md`](./staged-mcp-on-blox.md)),
`remote_staged_mcp::note_outbox::tests` (outbox parsing including a
marker that changed between the reads, transport vs content failures,
per-turn collection into the slot, start-of-run recovery that discards
unusable content, `write_note` run as a subprocess with fake JSON-RPC
including a rewrite blocked between its renames, local `sh` run of the
collect, clear and recovery scripts, schema parity with the Rust tool), `session_runner::tests::detached_recovery` (claim-first
sweep: one winner, resume during collection; the poll window and its marker;
a poll that finds the note claims with the storing marker, then ingests once,
then completes; two instances that read the row before either claimed settle
it once; a stale storing claim is polled again and the slow claimant's late
write is refused; a note that cannot be stored stays on the workstation in
both sweep paths; polls that find nothing write nothing; a row past the
window gets one last poll before the guarded expiry; an unreadable outbox
past the window is retried five times, then expired as unreachable; a
hold-stopped row with an empty note is polled while the note is empty and
only when it is remote, and drops out of the query once settled, a
late note fills it in under a storing claim without changing its status,
a poll without a note writes nothing and a resume wins the row, an expired
window settles it once and clears the outbox, an unstorable note keeps the
outbox, and the marker write takes only the observed row; only a
transport loss detaches, with bridge subprocess-exit wording excluded and the
innermost error in the message; an unacknowledged Stop and a watchdog hit
count as a lost agent, an acknowledged Stop does not, and an unacknowledged
interrupt counts only when the agent was silent throughout; the Stop copy
promises no note; the crashed row's innermost sentence; the timeline flag;
frontend marker parity for both suffixes and the Stop marker),
`remote_staged_mcp::note_outbox::tests` (a probe reports the note already
collected as pending until its stamp or content changes),
`session_runner::remote_note_watchdog::tests` (interval parsing, one probe
per silent interval until the note appears, an active agent is never
probed), `project_mcp::tests` (a completed child's status omits the sweep's
marker), `agent::activity::tests`,
`acp-client` driver tests (stdio injection and unattended mode for the
`goose` and `claude` providers, `initialize` capabilities; the transport-loss
phrase list in both directions; a Stop the fake agent never acknowledges
settles `CancelUnacknowledged` at the deadline while an acknowledged one
settles `Cancelled`; `session_isolation`: frames for another ACP session, or for any session
before the connection's own is named, are dropped by all three notification
handlers and are not activity, and a remote proxy group with a SIGSTOPped
child is killed whole — at once after an unacknowledged cancel, after the
grace when the wrapper ignores SIGINT, and right after the wrapper is reaped
when it honours it), `blox-cli`
(`--env-vars`), the session runner and prompt tests for the fallback and
follow-up paths, and on the frontend `toolCallViewModel.test.ts` /
`acpTranscript.test.ts` (the `write_note` row for both agents, interrupted
pending calls), `sessionModalHelpers.test.ts` (the detached and storing copy and which
end-of-session notice the chat pane renders), `acpTranscript.test.ts` (only
the final turn's unsettled calls are relabelled in a completed session) and
`sessionFailureCopy.test.ts` (the timeline copy says "yet" only while
`isPollingForNote` is set).
