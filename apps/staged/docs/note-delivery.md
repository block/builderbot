# Note delivery through `write_note`

A note session (a branch note, a project note, or a repo-note child of a
project session) writes its note into a per-session draft file and delivers
it by calling the `write_note` MCP tool. The file is the note; the call
publishes it. The file's first line is the note's title as a `# Title`
heading, and the body is everything below that line. Staged stores the
file's contents at the moment of the call, the title and the body apart, and
does not read the agent's assistant text. This replaced three earlier
contracts: the note as the text after a `---` separator in the final
assistant message, then the note as a `body` argument of the call, and then
the body in the draft file with the title as a `title` argument of the call.

The body moved out of the call because an argument cannot be edited. With an
inline body every refinement re-emitted the whole note: an amendment, a
Pikchr repair and a rewrite each cost the full note in output tokens, and
agents that drafted in a scratch file first paid for it twice. With the
draft file a refinement is an edit of the same note, and the call carries
two short strings.

The title moved out of the call for a related reason. While it was an
argument, the seeded draft of an amendment held the body only, so an
amending agent retyped the title from memory in the call. That could drift
from the stored title, and it overwrote the ` (2)` suffix that a move to
another branch can add. With the title in the file, the agent reads the
stored title and keeps it unless it edits that line, and the draft is the
text the note viewer renders and Copy produces. What is stored did not
change: the title and the body are still separate columns, `meta.json` has
no new key and there is no migration (see _Design decisions_).

## Tool contract

Staged offers a required MCP server named `staged` to every note session. It
exposes one tool:

```text
write_note(suggested_next_steps?: object | null)
```

- `suggested_next_steps` — `{ "suggestedNextCommitStep": string | null,
"suggestedNextNoteStep": string | null }`, the same shape the old
  `suggested-next-steps` JSON block carried. Omitting the argument keeps the
  steps already stored on the note (a rewriting turn without the block used to
  behave the same way).

That is the whole schema, and it requires nothing. There is no `body` and no
`title` argument, optional or otherwise: both come from the session's draft
file. The handler always reads the draft (`note_draft::read_draft` locally,
`readDraft` in the Staged MCP on Blox), takes the title from its first line
and publishes the rest as the body (_The title line_ below). The
draft must be a regular file, valid UTF-8, non-empty after trimming, and at
most 1 MiB (`DRAFT_MAX_BYTES`; the largest stored note is 145 kB). A draft
that is missing, empty, oversized, not UTF-8, not a regular file or
unreadable is a tool
error (`isError: true`) that names the file by absolute path and says what
to fix, for example ``write_note rejected: the draft file
`/…/draft.md` does not exist. This file is your note: write the full note
into it as markdown, starting with the title as a `# ` heading on the first
line, then call write_note again.``
A rejected call leaves the submission slot and the outbox untouched, so an
earlier publish in the same turn stands. A draft that does not open with its
title, or holds nothing below it, is rejected the same way.

Both handlers read the draft the same way, step for step:

- The file is opened once, non-blocking, and the type, the size and the
  bytes are all taken from that one handle. The file that is checked is the
  file that is read, so a path swapped after the check changes nothing. A
  FIFO at the path is rejected as not a regular file instead of blocking
  the handler on a writer that never comes. The read stops one byte past
  the cap, so a file that grows after the size check is still rejected as
  oversized.
- The open follows symbolic links. A draft that is a link to a regular file
  publishes that file's contents; a dangling link is a missing draft. This
  is not a boundary: the agent that could plant the link already has the
  session's file access and could have copied the file into the draft.
- One leading byte order mark (U+FEFF) is dropped before the trim. Some
  editors write one, and without this a draft holding only the mark would
  be published as an invisible three-byte note. A draft that is only the
  mark, or the mark and whitespace, is rejected as empty. A second mark, or
  one anywhere else, is text and is kept.
- The trim is Rust's `str::trim` (Unicode `White_Space`) on both sides. The
  script does not use JavaScript's `trim()`, which also strips U+FEFF and
  leaves U+0085, so the same file yields the same title and body on both
  transports.
- A file the handler may not read (mode 000, for instance) is rejected with
  ``write_note rejected: the draft file `…` could not be read: <cause>,
then call write_note again.`` on both; the cause is the platform's own
  message.

Each call republishes the file; the last call of the turn wins. The result
text is `Saved note "<title>" (<n> bytes from the draft file). If the user
asked you a question this turn and you have not answered it in chat yet,
answer it now, briefly. Otherwise send a simple "Done" message with no other
text to end your turn, rather than summarising the work done.` (`<title>`
is the title read from the draft's first line, which tells the agent what
was parsed. `<n>` is the UTF-8 length of the body, without the title line,
on both transports, which is why the text says bytes and not characters.)
The text is word for word the same on both
transports (`note_tool::saved_message`, `savedMessage` in
`staged-mcp.cjs`). The tool description ends with the same request: send
"Done" after the saved result, and answer a question before making the
call.

The call does not end the turn. Neither MCP nor ACP lets a tool declare
that it ends one, so the agent always gets one more response after the
result. The wording asks for that response to be the one word "Done", and
the chat pane hides it (_Ending a saved turn: the hidden "Done"_ below).
The message writer still treats a completed `write_note` call like the old
separator: the session finishes after the response that follows it instead
of holding for background work. That response is now the "Done" message.

No call, no note. A draft that changed without a `write_note` call is never
stored, whatever it holds: the call is the one signal that the note is
ready, and the one place the next steps come from.

### The title line

The title is the draft's first line. Both handlers split the draft by the
same rule (`note_draft::split_title` on the laptop, `splitDraftTitle` in
`staged-mcp.cjs`), applied to the text the read above returns, so a leading
byte order mark and the surrounding whitespace are already gone:

1. The first line is the text up to the first line ending. A line ending
   is a `\n`, a `\r\n` or a `\r` on its own, as in CommonMark.
2. It must be one `#` followed by a space or a tab. The title is the rest of
   that line, trimmed, and must not be empty.
3. The body is everything below that line, trimmed, and must not be empty.

Only that one line is consumed, and it is not part of the stored body. The
note viewer, Copy and the context given to later sessions all put the title
above the body themselves, so a body that kept the line would show the title
twice.

| Draft opens with                                 | Result                                                                           |
| ------------------------------------------------ | -------------------------------------------------------------------------------- |
| Blank lines or a byte order mark, then `# Title` | Accepted. The read has already removed them.                                     |
| `## Title`, or any deeper heading                | Rejected. A `##` heading is a section and never a title.                         |
| `#` alone, or `#` and spaces                     | Rejected.                                                                        |
| `#Title`, `#note:<id>`                           | Rejected. Neither is a heading, and a hashtag reference must not become a title. |
| `Title` over a line of `===`                     | Rejected.                                                                        |
| Front matter (`---`), or plain text              | Rejected.                                                                        |
| ``# Use `foo` for [bar](x)``                     | Accepted. The title text is kept as written, inline markdown included.           |
| `# Title #`                                      | Accepted as `Title #`. A closing `#` is not stripped.                            |
| `# #12 follow-up`                                | Accepted as `#12 follow-up`. A `#` the title text starts with is kept.           |
| `# Two  spaces`, or a title with a tab in it     | Accepted with that spacing. Whitespace inside the title is kept as written.      |
| `# Title`, a `\r` on its own, then the body      | Accepted as `Title`. The `\r` ends the title line.                               |
| `# One`, U+2028, `Two` on the same line          | Accepted as one title that holds the U+2028. It does not end a line in markdown. |
| `# Title` and nothing below it                   | Rejected as a note with no body.                                                 |
| A second `# Heading` further down                | Body text, untouched.                                                            |

The three line endings are read alike. A draft with CRLF line endings splits
the same way as one with line feeds. So does a draft with CR-only line
endings: its title line ends at the first `\r` and the rest is the body,
with its own line endings kept. A title therefore never holds a `\r` or a
`\n`.

The title text is not normalized. Tabs, runs of spaces, no-break and
ideographic spaces inside it are stored as written; only the ends are
trimmed. The characters that are a line break elsewhere but not in markdown
(U+2028, U+2029, U+0085, a vertical tab, a form feed) do not end the title
line either. Inside the title they are title text, and at its ends they are
trimmed with the other whitespace. A draft whose only line break is one of
them is a single line, so it is rejected as holding only the title line.

The two rejections use the frame of the other draft errors and name the
file by absolute path, word for word the same on both transports:

- No title line (`DraftError::MissingTitle`): ``write_note rejected: the
draft file `<path>` does not start with the note's title. Its first
non-blank line must be a level-1 markdown heading: `# ` followed by the
title on one line (not `##`, not a line underlined with `===`, and nothing
above it). Add that line at the top and leave the rest of the note below
it, then call write_note again.``
- Title only (`DraftError::TitleOnly`): ``write_note rejected: the draft
file `<path>` holds only the title line. Write the note body in markdown
below the `# ` title line, then call write_note again.``

**A stale `title` argument.** `title` was an argument until the title moved
into the draft. No schema and no prompt names it now, but a session resumed
with the earlier instructions in its context may still pass it. Both
handlers therefore still read it (`WriteNoteParams.title`, hidden from the
schema with `#[schemars(skip)]`, and a plain `args.title` read in the
script) and apply one rule (`note_tool::stale_title_agrees`,
`staleTitleAgrees`):

- Absent or null: the normal call.
- A string that, trimmed as Rust trims, is the draft's title as written, or
  is that title behind exactly one leading `#`: accepted, and the draft's
  title is what is stored. The second form is there because the earlier
  contract stripped one leading `#` from the argument, so a call that
  passed `# Plan` meant the title `Plan`. Nothing more is stripped:
  `## Plan` agrees with a draft that opens `# # Plan` or `# ## Plan` and
  not with one that opens `# Plan`.
- Anything else, an empty string and a value that is not a string included:
  rejected, and nothing is written. The text is ``write_note rejected:
`title` is no longer an argument. The note's title is the first line of
the draft file `<path>`, currently "<heading title>", which differs from
the `title` you passed. Edit that line if the title should change, then
call write_note again without `title`.``

The draft is checked before the argument is compared, on both transports,
so a call that passes `title` on a draft that is missing, empty or has no
title line gets that draft's error. A `title` that disagrees is rejected and
not ignored, because ignoring it would silently drop a rename asked for by
an agent whose context still holds the earlier instructions. The Rust field
is a JSON value and not a string, so a number or an array gets this
rejection, as in the script, and not a deserialization error.

This tolerance is temporary. It is to be removed from both handlers once
sessions started under the earlier contract are unlikely to be resumed, and
that removal has not been done.

### The draft file

Each note session has exactly one draft file, and every prompt names it once
by absolute path (`note_draft::prompt_draft_path`):

| Session                    | Draft file                                      |
| -------------------------- | ----------------------------------------------- |
| Local branch note, project | `<temp dir>/staged-note-draft-<session-id>.md`  |
| Remote (Blox) branch note  | `<home>/.staged/sessions/<session-id>/draft.md` |

`<temp dir>` is `std::env::temp_dir()`, the one place every local provider
writes without a prompt or an extra sandbox root (Codex included). `<home>`
is the workstation's real home directory, spelled out because Goose does not
expand `~`.

What the file holds at run start depends on the stored note
(`session_runner::stored_note_seed`, `note_draft::seed_body`):

- **Fresh note** (the stored note is empty): the file is not created. An
  empty file would trip Claude Code's read-before-write gate, and Write on a
  missing path needs no Read. A stale draft from an earlier run is removed.
- **Amendment or Pikchr repair** (the stored note has a body): the file is
  seeded with the stored note before the agent starts, so the agent reads
  it and edits only what changes. The seed is the stored title as the title
  line, a blank line, then the stored body verbatim
  (`note_draft::seed_body`, which gets the title line from
  `note_draft::title_heading`). Verbatim includes a heading the body opens
  with and every `staged-media://` reference. The seed and the publish
  mirror each other: the store keeps the body without the title, the seed
  adds exactly one line above it, and `write_note` takes exactly one line
  off. A body that opens with `# Section` keeps that heading: it is the
  draft's third line and never its first.

  The title is made to fit on one line before it is written
  (`heading_safe_title`), and only what the split could not read back is
  changed: a line break (a run of `\r` and `\n`, with any whitespace that
  touches it) becomes one space, the ends are trimmed, and a title with
  nothing left becomes `Untitled Note`. All other whitespace inside the
  title is written as stored, U+2028 and the like included. Whether there
  is a seed still depends on the body alone: a note whose stub row has a
  title and no body is a fresh note.

  A seeded draft published untouched gives the stored title and body back,
  so the store sees an identical republish and keeps `updated_at`. For the
  title this holds for every title `write_note` stores. Such a title is
  trimmed, not empty and holds no `\r` or `\n`, so the seed writes it
  unchanged and the split reads the same text back. There is one exception,
  on the body side, and it is older than the title line: a stored body with
  whitespace at its ends is seeded as stored and published trimmed.
  `write_note` stores a trimmed body, so this too concerns only a body
  stored some other way.

  A title stored some other way can still need the seed's change: one with
  a line break in it, with whitespace at its ends or with no text. Sources
  include an outbox from a script before version 10, whose title could
  hold a `\r` on its own (see _Collector_), and the `---` fallback, which ends
  the title at a `\n` only and can fall back to the start of the prompt.
  Such a title is changed by the first seed, to one that later seeds leave
  alone.

- **Repair of a waiting note** (remote only): when the start-of-run
  recovery finds a published note with a Pikchr block that does not parse,
  the draft that note was published from is neither removed nor seeded
  over, and the run opens with the repair request (_A waiting note with a
  broken diagram_ below).

When the run is over the local draft is removed. The remote draft is removed
with the outbox and kept in the states that keep the outbox (below).

The prompt builders (`build_full_prompt`, the project-session action
instructions and the note follow-up prompt) all use
`session_commands::note_delivery_guidance(draft_path)`. It names the file
once and states the contract in one sentence: "Your note draft file is
`<path>`. This file is your note. Write it, edit it, and when it is ready
call `write_note`." It says how the file is laid out ("The file's first line
must be the note's title as a level-1 heading: `# ` followed by the title on
one line. The note body goes below that line; use `##` and deeper headings
for its sections"; the advice about sections is guidance, which the handlers
do not enforce). It adds the amendment rule ("If the file already exists, it
holds the note as last saved, title line included: Read it and edit only
what changes instead of rewriting it"). It says that assistant text is not
saved, that neither the title nor the body is passed to the call ("both come
from the file"), that a published note or its title is changed by editing
the file and calling again, and that the call takes one argument. Its last
paragraph asks for the ending: once the tool
reports the note is saved, send a simple "Done" message with no other text,
which Staged hides, and answer a user's question in the reply before the
call. One wording serves a fresh note and an amendment because
the seed rule above decides whether the file exists. When the path is the
`$HOME/…` spelling of a workstation whose home could not be resolved (see
_Remote (Blox) sessions_), the guidance adds that `$HOME` there is a
placeholder: the agent must not use the literal text as a path, and must
resolve it first or take the absolute path from the `staged` server's
instructions.

The Pikchr correction prompt (`build_pikchr_correction_prompt`) carries
where the error was found (`PikchrErrorOrigin`), because the two checks
read different text and the line number counts that text's lines:

| Error found in                  | `write_note` attached | The prompt asks for                                                                                                                                                                                                                                               |
| ------------------------------- | --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| The note the call published     | yes                   | An edit of the block at line N of the note body in the draft file, then another `write_note` call with the suggested next steps. The line counts the published body, from the first non-blank line after the `# ` title line, so it is not the file's own line N. |
| The final assistant message     | yes                   | The diagram corrected in the draft file (written if it does not exist yet, the title line first and the body below it), then a `write_note` call. It says the line number is the reply's and makes no claim about what the draft holds.                           |
| The final assistant message     | no                    | A corrected reply in the `---` format. It says the tool is not available in this run and does not ask for a call.                                                                                                                                                 |
| A plain session's final message | n/a                   | A complete corrected final response.                                                                                                                                                                                                                              |

Which text is checked depends on what the turn will store
(`turn_pikchr_error`):

- **A `write_note` tool is attached and the slot holds a published note.**
  Only the published body is checked. The final assistant message is not
  saved in this state, so a broken `pikchr` block quoted in it (an agent
  explaining the block it just fixed is the likely case) is not an error:
  it spends no repair turn and cannot end a run whose published note is
  valid in `PikchrRepairExhausted`. The slot keeps the last published note
  across the turns of a run, so this also covers a repair turn once the
  note has been republished (locally) or re-collected from the outbox
  (remotely). A repair turn that did not republish is still judged by the
  note in the slot, and gets the prompt about the note again.
- **Anything else.** The final message is checked first, then the slot.
  This is a run with no tool attached (a plain session, or a note run on
  the `---` fallback, whose reply is the note) and a turn that published
  nothing, where the reply may be the fallback's note. An agent that echoed
  a broken diagram without publishing gets the prompt about the reply (the
  second row of the table).

Only an error in the published note may say "your draft file holds the note
you saved" and point at a line in it: a fresh note whose agent never wrote a
draft has no such file.

"`write_note` attached" is per run. It is false when the local server did
not start, or when the Staged MCP on Blox could not be installed, and such a
run can only deliver through the `---` fallback. The runner then has no
draft path to hand the prompt builders, so the first-turn notice, the
correction prompt and the follow-up (all below) describe the `---` format
instead of asking for a call that cannot be made.

The guidance names the
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
`staged: write note` from Goose, `mcp.staged.write_note` from Codex) is not
shown as a generic "Ran" row. A call that saved the note is shown as the
note card, "Note created" or "Note updated", in the row's place (_The note
card_ below). Every other call gets its own row (`noteToolCall.ts`,
`NoteToolDetails.svelte`): "Saving note" while the call runs, "Note
rejected" when the tool refused it, "Note not saved" for a cancelled call,
and "Saved note" for the one saved call that stays a row, a final-turn call
that did not change the stored note. The row is its verb alone, because the
call carries no title, and its details list the suggested next steps and
the tool result. A call with no arguments at all, which the schema allows,
has no note section, so its row expands only when there is a result to show
(`writeNoteCallHasDetails`). The row has no title and no body preview: the
stored note is where both are read.

A transcript recorded while `title` was an argument renders as it did. The
row is named after the title (`writeNoteRowDetail`), its details show the
title above the steps, and the raw input JSON stays hidden. A `body` on a
row recorded before the draft file is ignored.

Tool calls still `pending` or `in_progress` when the session has settled
render by the session's outcome (`AcpTranscriptOptions`): in a session that
ended `error` or `cancelled` they were cut off and read "Interrupted" in
the collapsed header as well as in the status; in a session that ended
`completed` — a remote note session completed from the workstation outbox —
their work stood, so other calls read "Finished with the session" and the
`write_note` call resolves to `completed`. It is then a saved call like any
other: the note card, or the "Saved note" row under the final-turn rule.
The backend does not rewrite their status; the transcript builder derives
it from the session status.

Source: `src-tauri/src/note_tool.rs` (schema, handler, in-process server),
`src-tauri/src/note_draft.rs` (paths, validation, error text, seed).

## Ending a saved turn: the hidden "Done"

A saved `write_note` call leaves the agent one more response, and Claude
spent it on a recap of the note it had just published. The texts the agent
reads about the tool now ask for one word in its place, and the chat pane
hides that word. A saved turn therefore shows the note card and nothing
under it. No hook, cancel or driver change is involved, and the mechanism
is the same for Claude, Goose and Codex.

The request is made in three places:

| Text             | Where                                                                   | What it asks                                                                                                         |
| ---------------- | ----------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| Tool result      | `note_tool::saved_message`, `savedMessage` in `staged-mcp.cjs`          | Answer the user's question now if it is still unanswered. Otherwise send a simple "Done" message with no other text. |
| Tool description | The `#[tool]` attribute, `WRITE_NOTE_DESCRIPTION` and the script's copy | Send "Done" after the saved result. Answer a question before making the call.                                        |
| Session prompt   | The last paragraph of `session_commands::note_delivery_guidance`        | The same request, and that Staged hides the message.                                                                 |

The result text carries the "answer it now" clause on purpose. In live runs
a wording that only said "answer before the call, then Done" lost the
answer in 8 of 38 turns where the user had asked a question. This wording
lost none in 27. A reply the agent writes after the call is real text, and
the pane shows it under the card.

Nothing forces the agent to comply. A turn that ignores the wording renders
as it did before the change, with its text shown, and is counted by the
drift log (_The drift log_ below).

The hiding rule is a pure module, `noteDoneMessage.ts`.

`isNoteDoneText` decides what a bare "Done" is. It trims the text, removes
one pair of wrapping `**`, `*`, `_`, backticks or quotes, removes one
trailing `.` or `!`, and compares the rest with `done`, ignoring case. So
`Done`, `Done.`, `done` and `**Done**` match. `Done ✅`, a sentence that
starts with the word, and an answer whose last line is `Done` do not.

`hiddenNoteDoneMessageIds` takes the transcript groups and returns the
assistant messages to hide. A message is hidden only when all of these
hold:

1. **A saved call comes directly before it.** The nearest earlier group is
   a tool group whose last `write_note` item resolved to `completed`. The
   item is found by tool name (`writeNoteToolCall`), not by its label, and
   the status is the resolved one. A rejected, running or interrupted call
   hides nothing, and a remote call completed from the outbox counts as
   saved. Other items in the group do not matter: Codex completes a review
   item after the call, and Claude sometimes issues a shell call beside it.
2. **Only such messages follow the call.** Every assistant message between
   that tool group and the end of the turn is a bare "Done". A background
   task that wakes the session can add a second one, and both are hidden.
   A recap after the first shows both messages.
3. **The turn ends there.** What follows is the next user message or the
   end of the transcript, not another tool call.

The rule fails open. On any mismatch the message renders whole, exactly as
the agent wrote it, and nothing is trimmed from it. The worst case is a
visible "Done".

The row stays stored. `SessionChatPane` derives `renderedGroups` from the
transcript groups and leaves a hidden group out of the list. It does not
render the group empty, because an empty group would take a second gap in
the column and be picked up as a scroll anchor. The rule runs on every
rebuild, so a message that grows past the word while streaming comes back
with all of its text.

The stored row is still read elsewhere, and none of these readers filters
it:

- **Unread chat and note freshness.** The backend queries behind a note
  row's chat button count visible assistant rows newer than the note
  (`VISIBLE_ASSISTANT_FILTER`), and they count a "Done" row. A turn that
  changed the note stores it after the turn, so the note is newer than its
  own "Done" and nothing changes. A turn that republished an identical
  note keeps the note's `updated_at`, so its "Done" is newer than the note
  and the chat reads as unread, as the recap row did before. The forced
  republish after a chat answer was the main source of an identical
  republish, and the fallback rule removed it (_Fallback: no `write_note`
  call_ below).
- **`wait_for_repo_session`.** It returns the last assistant message as
  `output` beside the note. A project agent reads "Done" there in place of
  a recap. The note itself is in the same payload.
- **The live activity hint.** It shows the latest assistant text, so it can
  read "Done" for the moment between the message and the end of the turn.

Remote sessions follow the same rule over the same rows. The script's
result text is compared with the laptop's by test, and `SERVER_VERSION` went
to 7 for this wording (see [`staged-mcp-on-blox.md`](./staged-mcp-on-blox.md)).
The upload is gated on a checksum, so a workstation gets the new text on
its next session, together with the app that hides the word.

### The note card

The chat pane marks each point where a note was delivered with a card:
"Note created" for the first delivery in the transcript and "Note updated"
for every later one. `listNoteDeliveries` in `noteDeliveries.ts` decides
where they go, from the transcript groups alone. Nothing in the backend
records "the note was updated". A delivery has one of two shapes:

- **A `write_note` call that resolved to `completed`.** This is the tool
  path. The card is drawn in place of the call's tool row, since a "Saved
  note" row with a card under it would say the same thing twice. The call
  is found by tool name and by its resolved status, as in the hiding rule.
- **An assistant message that holds the `---` separator.** This is the
  shape of a run with no tool attached and of every session recorded before
  the tool. The card sits under the text above the separator, and reads
  "Writing note" while the session runs and the message is the last one in
  the transcript. A call is never "Writing note": its "Saving note" row
  says so while it runs.

The first delivery is "Note created" across both shapes, so a session that
began on the separator and continued on the tool has one "Note created".

A call that was rejected, interrupted or is still running is not a
delivery. It keeps its tool row, with its details. A rejected
call followed by a retry shows a "Note rejected" row and then the card. Two
saved calls in a row are two cards: `groupRichToolsByVerb` takes a
predicate for the items the pane renders on its own, so they do not fold
into one collapsed "Saved note" row.

**The final-turn rule.** One saved call is not a card. The rule applies in
the final turn of a session that is no longer running, when the pane is
opened on its note. There a saved call is a card only when `updatedAt` on
the note is at or after the call row. The note is stored after the turn, so
a turn that changed it stamps it later than its own call. An older note was
not changed by that call: the agent republished an identical note, which
keeps its timestamp, or the run failed before the note was stored. That
call keeps its "Saved note" row. In a completed session the "Ask for the
note to be updated" button shows under it, because `shouldAskForNoteUpdate`
reads the same timestamp. The card and the button therefore never show
together for one turn.

The final turn is everything after the last user message, as in the
transcript builder. Calls of earlier turns are not compared, because a
later turn has moved the note on since. A pane with no note context
compares nothing, and every saved call there is a card.

The comparison waits for the session to settle. The note cannot be stored
before the run ends, and a run can wait on background work after the call.
A strict reading would hold the row for the whole turn and swap it for the
card at the end. So while the session runs, a saved call is a card as soon
as it completes.

### The drift log

A model that stops following the wording goes back to writing a recap under
the card, and no test or error says so. After each settled turn of a note
session the run loop therefore calls
`note_done::log_settled_turn_note_done_drift`. It logs one line at info
level when the turn saved a note and did not end in a bare "Done":

```text
Session <id>: note-done drift: the turn saved a note and did not end in a bare "Done": 1 message after the saved write_note call, the last 742 chars
```

The line holds counts and lengths only, never the text. After the fixed
phrase `note-done drift` it says what followed the last saved call:

| Ending of the line                                                   | What happened                                                                 |
| -------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `no message after the saved write_note call`                         | The turn ended at the call, with no closing message.                          |
| `<n> message(s) after the saved write_note call, the last <m> chars` | Text followed the call. `<m>` is the trimmed length of the last message.      |
| `…, ending in a "Done" line`                                         | The last message is a longer answer that the agent closed with a `Done` line. |
| `…, with a tool call after the text`                                 | A tool call came after the closing text, so the turn did not end there.       |

The decision is `note_done_drift`, a pure function over the turn's stored
rows. It follows the pane's rule:

- The turn is everything after the last user row. A message steered in
  after the call starts a new turn, and what the agent wrote after it is
  not checked.
- The turn saved a note when the run's submission slot is filled and the
  turn holds a `write_note` call. The slot alone outlives a turn, so on its
  own it would match a repair turn that never called the tool.
- After the last such call the tool rows beside it are stepped over. Then
  at least one assistant message must follow, each must be a bare "Done",
  and the turn must end there.

The call sits after the outbox collection, so a remote note is in the slot,
and before the Pikchr check, so a turn that goes on to a repair is counted.
The log and the pane differ in one rare case: a saved call followed by a
rejected one in the same turn. The pane then shows the closing word and the
log reports nothing.

**What a rising count means.** Count the lines that hold `note-done drift`
and read the count against the number of saved turns. A single line is not
an error. The result text itself asks for an answer in place of "Done" when
a user's question is still open, and that turn is logged like any other. A
steady low count is those turns. A count that rises with no change in how
people use note sessions means the wording has stopped working: an agent or
a new model is writing after the saved call again, and users see a recap
under the card. The lengths tell the two apart, since an answer is short
and a recap ran to a median of 186 words in stored sessions. When the count
rises:

- Run the ignored real-agent test against each bridge (_How to test
  manually_ below) to see which agent stopped complying.
- Change the wording in all three texts together. `noteDoneWording.test.ts`
  fails when one side changes alone or quotes a word the filter does not
  accept.
- If no wording holds for Claude, add the `PostToolUse` hook that was
  planned as the hard stop and is held in reserve. The wording and the
  filter stay in place under it.

The log and the filter cannot disagree about what "Done" is or which call
is `write_note`. `note_done/rules.rs` holds Rust copies of the pane's
`isNoteDoneText` and `isWriteNoteToolName`, with JavaScript's white space
set for the trim. Both languages run the same case table,
`src/lib/features/sessions/testStubs/noteDone.cases.json`, so a change to
one side alone fails a test.

Source: `src/lib/features/sessions/noteDoneMessage.ts` (the hiding rule),
`src/lib/features/sessions/noteDeliveries.ts` (the card),
`src-tauri/src/note_done.rs` and `src-tauri/src/note_done/rules.rs` (the
drift log).

## Local and project sessions

Locally the server is an in-process rmcp streamable-HTTP server bound to
`127.0.0.1:0`, exactly like the `project` and `pikchr` servers, and is passed
to `session/new` as `McpServer::Http`. The handler is bound to the session's
draft path when the server starts, and names it in the server's `initialize`
instructions (`note_tool::draft_instructions`) with the sentence the Staged
MCP on Blox uses. The tool description says the path "is in your
instructions", and this is what makes that true on both transports, even
for a session whose prompt predates the draft file. On each call it reads the draft,
splits it into the title and the body (`NoteSubmission::from_draft`), and
writes them with the next steps into a per-run `NoteSubmissionSlot`
shared with the message writer, the post-turn Pikchr validation (which
checks the published body, since the transcript does not carry the note,
and skips the unsaved reply once a note is published),
and the post-completion hooks.

The draft's run lifecycle is three steps in the session runner:

1. Before the server starts, `prepare_local_note_draft_for_run` seeds the
   file from the stored note or removes a stale one (the seed is written
   through a temp file and a rename, so the agent never reads half of it).
   A failure here is logged, not fatal: the tool reports a missing draft by
   path and the agent writes it.
2. During the run the file stays in place across turns, so a Pikchr repair
   turn or a `write_note` follow-up turn edits the file the previous turn
   published.
3. After the run's last turn `remove_local_note_draft_after_run` removes it,
   whatever the terminal status. The slot already holds the body the hooks
   store; the next run seeds again from the stored note.

A project session fixes its session id before it builds the prompt, so the
prompt can name the draft by session id. Only the id exists at that point:
the `Session` value is built in memory first, the prompt is written into
it, and the row is inserted afterwards.

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
   that is empty or more than one line) is logged, cleared and skipped, because nothing
   will ever make it ingestible. A waiting note is checked before it is
   handed to the hooks: one with a Pikchr block that does not parse is not
   stored. It is reported as `OutboxRecovery::Rejected` with the parse
   error, and the run that is starting is the repair (see _A waiting note
   with a broken diagram_ below). This is where a row the detached sweep
   parked on a broken diagram ends up when the user resumes it, and it is
   the read that would otherwise store a note an earlier run had rejected,
   had republished still broken, and then never read again (see _Clear
   unless republished_ under _Collector_). Either way the server is
   still attached and
   the run starts: a recovery failure must not cost the session its tool.
   The command that empties the outbox also puts `draft.md` in its
   run-start state (`clear_outbox_and_seed`): removed for a fresh note,
   written from the stored note, title line first, for an amendment. It leaves the session
   directory in place (mode 700) either way, so the agent's first write of
   the draft needs no `mkdir`. The seed is asked for after any ingest, so a
   note recovered a moment ago is what gets seeded. The recovery reports
   two things separately (`RunStartRecovery`): what it found in the outbox,
   and how the draft came out (`DraftSeed`). A seed that could not be
   written is not a failed recovery. By then the outbox was read, any
   waiting note ingested and the files removed, so the run counts as
   settled and its end-of-run clear is unchanged; the seed failure is
   logged, the agent finds no draft, and `write_note` names the path to
   write. The seeding command always exits 0 and prints
   `staged-draft-seed-failed` when the seed failed, which is how a failed
   seed is told from a command that never ran;
3. injects the server into `session/new` / `session/load` with this run's
   turn id.

Steps 1 and 2 are two `ws exec` round trips before the agent starts (a
third and fourth when the script has to be uploaded or an uncollected note is
waiting), each a few seconds on a cold `sq blox` connection. That is the
price of a session whose deliverable survives the laptop disconnecting. The
draft adds none: the seed travels base64-encoded inside the clear command,
and only a seed over 100 kB encoded (a stored body over about 75 kB) is
appended in further commands of that size. The bound is Linux's: a
workstation refuses any single command argument of 131072 bytes or more,
whatever its `ARG_MAX`, and macOS has no such per-argument limit, so only a
Linux shell shows a chunk that is too large. The seed is decoded into `.draft.md.seed`
beside the draft and renamed into place. A chunked seed that fails part-way
leaves that temp file behind, so every clear script removes it along with
the draft; otherwise the session directory could never be removed.

**A waiting note with a broken diagram.** The recovery does not store such
a note, and it does not throw it away either. It used to clear the note
with its draft and tell the agent nothing, so the agent wrote the note
again from memory. Now:

- one command removes the publication only
  (`reject_published_note_script`): `done` first, then `meta.json` and
  `note.md`. The run that is starting must not collect the old note as its
  own, and no later reader may find it;
- `draft.md` is not touched and the seed is not asked for
  (`DraftSeed::Kept`), so the file is as the earlier run left it. That is
  normally the text the note was published from. It can also be later text,
  when a repair turn fixed the block and lost its connection before it
  called `write_note`. When the earlier run left no draft at all, the same
  command builds `draft.md` from the rejected note, so the file the prompt
  names exists and is one the tool accepts. `note.md` holds the body only,
  so the command puts the title line for the rejected title
  (`RejectedNote.title`, through `note_draft::title_heading`) and a blank
  line above it, in the seed's temp file, and renames that into place. A
  `write_note` call on that draft publishes the rejected title and body
  again. A draft kept from a run that started before the title moved into
  the file has no title line: the repair run's first call is rejected for
  the missing line, and the agent adds it;
- the rejected note (`RejectedNote`: its title and the first block that
  does not parse) is carried to the run's first prompt. `first_turn_prompt`
  appends a short notice of what Staged found and then the correction a
  live run sends for a submitted note, which names the draft file and the
  block's line in it and asks for an edit there and another `write_note`
  call. Like the no-tool notice, this goes to the agent only, and the stored
  first user message is the prompt as built;
- the run counts as settled for the end-of-run clear, as after any
  recovery that saw the outbox through. The kept draft therefore follows
  the usual policy: it goes with the outbox when the run ends in a state
  that clears it.

If the removal command cannot be confirmed, the recovery is an error like
any other failed clear: the run is unsettled, nothing is appended to the
prompt, and the note stays for the next attempt.

**The session directory exists from run start.** Before the draft file,
the directory was created by the first `write_note` call. Now the run-start
clear creates it, so a session whose agent never publishes still has an
empty directory, or one holding an unpublished `draft.md`, until something
clears it. In the two states that keep the outbox (below) that is what
stays on the workstation: a lost connection with no note leaves the
directory and whatever draft the agent had written, and so does a run whose
recovery did not settle. The next Resume's recovery, or the sweep when it
stores a late note or expires a hold-stopped row, removes them. Nothing
reads the directory in between except the collectors, which look only for
`done`.

**When the run-start recovery does not settle.** If step 2 fails before its
clear (the outbox could not be read, or a waiting note could not be
stored), the draft is neither removed nor seeded, and the server is
attached anyway. `draft.md` is then whatever the lost run left: nothing, the
note that run published, or text it wrote and never published. The prompt's
"if the file already exists, it holds the note as last saved" is not true of
the last case. No round trip is spent on fixing that, for two reasons.
Nothing in that state can become the note without a call: every reader on
the laptop (the per-turn collector, the watchdog's probe, the sweep, the
detached and hold-stopped polling, the next recovery) reads `note.md`
beside a `done` marker and never the draft, so an unpublished draft is
stored only if this run's agent calls `write_note` on it. And the usual
outcome is benign: the agent resumes its own session and carries on from
its own earlier draft. A missing draft is the tool's "does not exist"
error, which the agent fixes by writing the file.

The prompt needs the draft's absolute path before the run starts, so the
prompt builder resolves the workstation's home directory itself, in a
command it already runs: the Pikchr grammar upload's last `ws exec` ends
with `&& printf '%s' "$HOME"` (`write_bytes_to_remote_then_with`,
`note_draft::REMOTE_HOME_EPILOGUE`). A note prompt with no grammar to upload
asks in a `ws exec` of its own, and a workstation that reports no home gets
the path spelled `$HOME/.staged/sessions/<session-id>/draft.md`, logged at
warn level. That spelling is a placeholder, not a path. A file tool that
does not expand variables would treat it as relative and create a `$HOME`
directory inside the worktree before `write_note` rejected the call. So
`note_delivery_guidance` adds a paragraph whenever the path starts with
`$HOME/` (`note_draft::names_unresolved_home`): do not use the literal
`$HOME/...` text as a path; resolve `$HOME` first, or take the absolute
path from the `staged` server's instructions. The Staged MCP on Blox names
the absolute path in its `initialize` instructions and in every draft
error. The runner takes the same home from the install probe
(`RemoteStagedMcp::home`) for the correction and follow-up prompts.

On the workstation `write_note` reads `draft.md` from the session's scratch
directory and writes atomically (temp file + rename) into the outbox, the
note's three files beside it:

```text
~/.staged/sessions/<session-id>/draft.md   the agent's draft: the whole note in markdown, the `# Title` line first
~/.staged/sessions/<session-id>/note.md    published body: the draft at the call, without the title line
~/.staged/sessions/<session-id>/meta.json  {"title","suggestedNextSteps","turn","writtenAt","helperVersion"}
~/.staged/sessions/<session-id>/done       marker holding the call's token, removed first and written last
```

`note.md` is a snapshot, not a link: the collector reads the note the call
published even if the agent goes on editing the draft, and the draft is
left in place after the call so a second call republishes it.

The script splits the draft on the workstation. `meta.json` gets the title
read from the draft's first line, and `note.md` gets the body below that
line. The split has to happen there: the laptop never reads the draft and
may be disconnected when the call is made, and a draft with no title line
has to be answered with a tool error in that call. The outbox format did not
change when the title moved into the draft. It is the same three files, and
`meta.json` has the same keys.

A rewrite (a second `write_note` call in the same turn) unlinks `done` before
it touches `note.md`, and recreates it only after both files are renamed into
place, so the marker is absent for the whole of a rewrite. Every call also
writes a new random generation token into the marker, 32 hex characters. A
script from before version 6 left the file empty.

The token is there because a reader takes several steps to read the three
files, and the sweep can land at any moment. A whole second call can fit
between the read of `meta.json` and the read of `note.md`, and the marker is
present both before and after it. Presence alone would then pair the first
call's title and suggested next steps with the second call's body. The
collector reads the marker on both sides of the files and takes them as one
note only when the token is the same (see _Collector_ below).

The `turn` value is the per-run id the server was started with, so the
collector can tell this run's note from an earlier one. `helperVersion` is
the script's `SERVER_VERSION`, under the key earlier builds wrote. The
collector reads it to decide how to take the title (_The title depends on
the script's version_ below).

### Collector

When a turn settles cleanly, `note_outbox::collect_turn_into_slot` runs
one `ws exec sh -c` that prints the marker line, `meta.json` and `note.md` as
base64 lines (base64 keeps arbitrary markdown out of the line protocol), and
then the marker line again. A marker line is `pending` when there is no
`done` file, and otherwise `done` followed by the token in it. The script
reads the marker with one `cat`, so presence and content come from the same
open, and cuts the token down to letters and digits so the file cannot add a
line to the output.

The files count as one note only when both marker lines say `done` with the
same token (`parse_collect_output`). Anything else is treated as `Pending`
and read again later rather than trusted:

- a marker missing at either read: `write_note` unlinks it while a rewrite
  is in flight, so the files may be half replaced;
- two different tokens: a whole `write_note` call ran between the two marker
  reads, so the title read first and the body read second may be from
  different calls.

Two empty tokens count as the same marker. That is a note published by a
script from before version 6, which is collected as it always was, without
the second protection. One empty token and one that is not are different
markers. The clear scripts only test that the marker exists, so they are
unchanged and work with either script.

**The title depends on the script's version.** `outbox_state` reads
`helperVersion` from `meta.json` before it builds the submission:

- **Version 8 or later** (`TITLE_AS_WRITTEN_SERVER_VERSION`): the title in
  `meta.json` is the one to store. It is trimmed and checked, and nothing is
  stripped from it (`NoteSubmission::from_parts_as_written`). From version
  9 it is always the text of the draft's title line, which the local handler
  keeps as written, so a draft that opens `# #12 follow-up` is titled
  `#12 follow-up` on both transports.
- **Version 7 or earlier, or no version**: the title is the `title`
  argument of the earlier contract, and `note.md` is the whole draft. The
  title is normalized the way the tool normalized that argument
  (`NoteSubmission::from_parts`: trimmed, one leading `#` stripped), so such
  an outbox is collected as it always was.

Both paths reject an empty or multi-line title and an empty body, with the
same reasons (`outbox note is invalid: …`). A title is multi-line when it
holds a `\n`, on both paths. A `\r` on its own inside the title is not
rejected at any version: the title is stored with it. Which scripts can
write one:

- **Version 7 or earlier**: in the `title` argument, in which those scripts
  rejected a `\n` only.
- **Version 8**: in the `title` argument, as before, and in a title read
  from the draft. That script ended the draft's title line at a `\n` only,
  so the draft `# Title\rBody line`, a blank line, then `More` was titled
  `Title\rBody line`.
- **Version 9**: in a title read from the draft, for the same reason.
- **Version 10 or later**: never. The title line ends at a `\r` too.

The collector does not reject such a title, because a rejected outbox is a
content failure and the run-start recovery clears an outbox that fails that
way, so the note would be lost. The title needs no rejection to be
repaired: the next seed of the draft writes
its line break as one space, as it does for any stored title with a line
break (see _The draft file_).

The gate is needed because outboxes of the earlier shape are still written
after an update. A running `node` process keeps the script it was started
from, so a turn that outlives its connection, or is taken over, finishes on
the script and the prompt it started with. Stripping a `#` from every title would instead have
titled `# #12 follow-up` as `12 follow-up` for a remote session only.
`outbox_state` is the one place a snapshot becomes a submission, so the
per-turn collection, the sweep and the run-start recovery needed no change
of their own.

A `Delivered` result for this run's turn id becomes a `NoteSubmission` with
source `Outbox` and is stored in the same `NoteSubmissionSlot` the local tool
fills. From there it feeds, with no further round trip:

- the post-turn Pikchr validation (`validate_submitted_note_pikchr`), so a
  remote note with a broken diagram gets the same correction prompt a local
  one does; the repair turn's rewrite is collected again when that turn
  settles, and the loop gives up after the usual number of attempts. With a
  collected note in the slot the reply is not checked, exactly as locally;
- the "did this turn deliver a note" decision behind the `write_note`
  follow-up prompt, so a remote turn that did call the tool is never asked
  to call it again — the follow-up fires only when the outbox has no `done`
  for this turn _and_ the `---` fallback found nothing _and_ the note has
  no stored body (see _Fallback: no `write_note` call_);
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

A turn whose `sq blox acp` proxy dies while bloxlet still runs the agent is
not one of those endings. The connection reports it as `Remote agent
detached: …` (`acp_client::is_remote_agent_detached_error`), which is not a
lost agent, and the run picks the turn back up over a fresh proxy and waits
it out (`pick_up_detached_turn`, `reattach_detached_turn`). That happens
inside the turn loop, before the loop looks at the turn's result, so a turn
that ends on the reattach is an ordinary turn end: the collection above, the
Pikchr check, the follow-up and the hooks all run for it, and the row never
leaves `running`. _A turn that outlives its connection_ below covers the
cases where the run itself is gone.

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

The end of a run decides what happens to the outbox
(`remote_outbox_clear_at_run_end`, `RemoteOutboxUse`, `RunEndOutboxClear`).
It acts with one more `ws exec` after the terminal status is written, so it
never holds the row in `running`. A clear removes `draft.md` and the seed's
temp file `.draft.md.seed` along with the three outbox files, and the
now-empty `~/.staged/sessions/<session-id>/` directory too. A run only
removes a published note it has dealt with:

- **Clear.** The run read the outbox after its last turn ended
  (`RemoteOutboxUse::read_after_last_turn`): a clean turn (note ingested or
  not), an exhausted repair loop, and the lost-agent and hold-stopped endings
  whose collection after the run went through. What was there has been
  stored, or judged and rejected. A Stop the user asked for clears as well,
  read or not, because it discards the turn's output (see _Design
  decisions_).
- **Clear unless published** (`note_outbox::clear_unpublished_outbox`). The
  run ended without that read: its last turn crashed, the agent cancelled its
  own turn, or a quit cancelled it. One command on the workstation tests
  for `done` and removes the files only when the marker is absent, so there
  is no window between a read and the clear. A published note stays whole,
  with its draft, and the next run's start-of-run recovery ingests it. These
  runs used to clear the outbox outright, which could delete a note nobody
  had read.
- **Clear unless republished** (`RunEndOutboxClear::ClearUnlessRepublished`).
  The same unread ending, for a run that had already read a note and turned
  it down: a Pikchr block did not parse, the run asked for a repair, and the
  repair turn ended with no read (a usage limit, expired auth, a turn the
  agent cancelled). The published note on the workstation may still be the
  rejected one, which is known bad and is removed at once. The
  collector records a stamp of every note it reads
  (`note_outbox::PublishedNoteStamp`: the POSIX `cksum` checksum and byte
  count of `meta.json` and of `note.md`), and `rejected_remote_note` hands
  the clear the stamp of a remembered note that fails the check. The same
  single command then compares `cksum` of the two files with the stamp: a
  match is the rejected note and is removed with the rest, and anything
  else is a note the repair turn published before it failed, which nobody
  has read and which stays. `meta.json` carries `writtenAt`, so a repair
  that republished the same body is still a different note. A note the run
  read and found valid is never treated as rejected. The stamp cannot
  recognise a repair turn's republish that is still broken; that note stays
  as unread, and the next run's start-of-run recovery checks it, does not
  store it, and has the agent repair its draft (above). The stamp used to
  be what kept a rejected note away from the detached sweep, which stored
  what it found unchecked. The sweep checks now (_Polling a detached row_),
  so a rejected note left on the workstation would be turned down again,
  not stored. The stamp clear is left as it was; removing it is a separate
  cleanup.
- **Keep.** Five states leave the outbox and the draft untouched:
  - a run that never attached the Staged MCP on Blox, which has no outbox;
  - a lost connection with _no_ note found, because the agent may still
    write into it and the sweep (or the next run's start-of-run recovery) is
    what reads it;
  - a run whose start-of-run recovery did not settle — the workstation
    dropped between installing the Staged MCP on Blox and reading the
    outbox, or a waiting note could not be stored — and that never collected
    its own note. (A draft seed that failed after the outbox was read and
    cleared is not this state: `remote_outbox_recovery_settled_for_run`
    counts that run as settled. Treating it as unsettled kept the outbox
    after a Stop that followed `write_note`, and the next run's recovery
    then ingested the note the Stop was meant to discard.) An earlier run's
    finished note may still be sitting there unread (the per-turn collector
    reports it as a stale turn and ignores it), and the clear would delete
    the only copy; the next run's recovery gets another chance. Once the run
    collects its own note the earlier one has been overwritten, and the
    clear is back on;
  - a collected note the hooks could not store (a media ingest or a store
    write failed): the outbox is the only copy, and the next run's recovery
    ingests it;
  - a valid collected note the hooks never ran for
    (`RunEndForOutbox::collected_note_hooks_skipped`): a quit cancelled the
    run between the read and its terminal state, so the row ended
    `cancelled` / `app_quit` and nothing was stored. The note was read but
    neither stored nor rejected, and the next run's recovery ingests it. A
    Stop the user asked for still clears.

A completed run whose read after the last turn failed is not left to that
policy (`completed_run_outbox_unread`). Nothing is known about what the
turn published, and completing would store an empty note while a published
one may sit on the workstation with nothing looking for it. The run reads
the outbox once more after its loop and settles from that read
(`unread_outbox_settlement`, `OutboxSettlement::TurnEndedUnread`), by the
rule a takeover uses for an outbox it cannot read (_A turn that outlives
its connection_):

- still unreadable: the row ends `error` / `detached` with the polling
  marker, the outbox is kept, and the detached sweep stores the note once
  the workstation answers (_Polling a detached row_). No empty note is
  stored;
- a published note: it never went through the loop's Pikchr check, so it
  is checked here (`unread_outbox_terminal_state`). A valid one completes
  the turn and is stored. A broken one ends the row `error` / `detached`
  without the polling marker and is cleared;
- nothing published: the run completes as a turn that delivered nothing
  does, and the outbox is cleared.

No connection was lost in this state and the agent is not still running, so
the two `detached` rows have their own text instead of the lost-connection
messages. The parked row reads "This session's turn ended on `<ws>`, but
Staged could not read its note from the workstation (`<read error>`); …"
and still ends with the polling marker, which is what the sweep and the
frontend key on. The phrase "could not read its note from the workstation"
is a second marker (`UNREAD_OUTBOX_DETAIL_MARKER`, mirrored as
`REMOTE_NOTE_UNREAD_OUTBOX_MARKER` and pinned by the same backend test):
the chat pane then says the turn ended and that Staged keeps checking,
instead of that the connection was lost and the agent may still be
running. The broken-diagram row reads "This session's turn ended on
`<ws>`, but the note it saved has a Pikchr block at line N that does not
parse (…); that note was not stored. Resume to have the agent write it
again." The sweep's later messages for a parked row (the expired window,
the unreachable workstation) are unchanged.

A Stop the user presses while that second read is out wins over whatever
it returns (`user_stop_requested`, asked again after the read; the first
sample is from before it). The read is a round trip to a workstation that
has just failed to answer, with the row still `running`, and nobody else
records that Stop. The run ends as a Stop during the turn does:
`cancelled`, no hooks, and the end-of-run clear discards the outbox. It
does not settle from the read, and it does not complete under the
late-cancel rule (`completed_turn_survives_late_cancel`) with the note the
read had just put in the slot, which the loop never checked. A quit that
lands during the read is not a Stop: the run settles from the read, and
the sweep recovers a parked row after the relaunch.

A failed read inside the loop is not final while a `write_note` follow-up
is due (a note with no stored body, and an attempt left): the turn counts as
one that delivered no note, the follow-up runs, and its turn is read again.
An amendment gets no follow-up, so its failed read is the run's last. The
rule above applies to the read after the run's last turn.

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
hold as soon as the response after it completes. That response is the
closing "Done" message, or the answer the result text asks for in its
place. On the Claude bridge the finish is taken at a successful result with
non-empty final text (`response_completion.rs`), which the one word
supplies. Remote sessions have no such
mid-turn signal: the outbox is only read after the turn settles, and the
prompt tells the agent not to write `---` text, so `should_finish_after_response`
never fires for them. A remote note session under a background hold therefore
runs to the hold's quiescence timeout (or its cap) even when the note was
saved early. Polling the outbox during the turn would cost a `ws exec` per
poll; this is a known limitation, not an oversight. Background work that
wakes the agent during such a hold can draw a second "Done" message after
the first. The chat pane hides a run of them (_Ending a saved turn: the
hidden "Done"_ above). A hold the user _stops_
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
separator text, see below) and the note has no stored body yet, the session
runner sends one follow-up prompt (`note_write_followup_prompt`): "Write or
finish the draft at `<path>` (the full note in markdown: the title as a `# `
heading on the first line, then the body), then call the `write_note` tool
with `suggested_next_steps`". A draft the agent wrote but never published is
covered by the same prompt: it only has to call the tool. It does this once;
a second silent turn stores the empty note as before, so a session cannot
loop.

The follow-up is a guard against storing an empty note, so a note that
already has a stored body gets none (`note_write_followup_due`,
`stored_note_has_body`). A turn there that publishes nothing is a chat
answer, or an amendment the agent chose not to make. The post-completion
hooks keep the stored note for it, and its `updated_at` does not move. The
chat pane reads that as a note older than the chat and offers "Ask for the
note to be updated" (`shouldAskForNoteUpdate`); the user decides whether the
answer belongs in the note. A forced republish would cost an agent turn for
every chat question and restamp a note that did not change. "Has a stored
body" is the reading the draft seed uses (`stored_note_seed`): a run whose
draft was seeded with the stored note is a run that gets no follow-up. One
gap is accepted: an agent that edits a seeded draft and never publishes it
is not prompted, and the next run seeds the draft from the stored note
again.

A run with no `write_note` tool attached (the local server did not start,
or the Staged MCP on Blox could not be installed) gets a different
follow-up, because the call it would ask for cannot be made: "The
`write_note` tool is not available in this run, so Staged takes the
note from your reply instead. Reply with a line containing only `---`, then
the note: a markdown H1 title (`# <Title>`) on the next line, followed by
the full note body". That is the format the fallback parser below accepts.
It is also the layout of the draft file, so the follow-up adds that an agent
which already wrote the note into its draft can use the file's contents as
the note.
The sentence says "in this run" on purpose: it stays in the agent's context,
and a later Resume of the same session may attach the tool.

The session's instructions describe the suggested next steps only as
`write_note` arguments, so the follow-up also says where they go in a reply
(`note_separator_fallback_next_steps`): above the `---` line, in the fenced
block `extract_suggested_next_steps` reads. It gives the rules that parser
applies and shows one block
(`NOTE_SEPARATOR_FALLBACK_NEXT_STEPS_EXAMPLE`):

````
```suggested-next-steps
{"suggestedNextCommitStep": "Implement this plan", "suggestedNextNoteStep": null}
```
````

The opening fence line ends right after `suggested-next-steps`, the closing
fence is three backticks alone on an unindented line, and the block holds one
JSON object with the two keys the tool's `suggested_next_steps` argument
uses, each a few words or null. The block is removed from the reply before
the note is read, so it never lands in the note body.

Such a run should not need the follow-up at all. The session's first prompt
is built in `session_commands` before the run knows whether the tool will
attach, so it names the draft file and `write_note`. When neither attached,
the runner appends a notice to that prompt before it is sent
(`first_turn_prompt`): your instructions name a draft file and the
tool, neither can be used in this run, do not write the draft file, and
"The `write_note` tool is not available in this run, so Staged takes
the note from your reply instead", followed by the same `---` format and the
same paragraph and example block for the suggested next steps. The
sentence, the format and the next-steps paragraph are the follow-up's own
(`NOTE_TOOL_UNAVAILABLE`, `NOTE_SEPARATOR_FALLBACK_FORMAT`,
`note_separator_fallback_next_steps`). The first turn
can therefore deliver, and the single follow-up stays in reserve for a turn
that still delivered nothing. On a workstation it also keeps the agent from
leaving a `draft.md` that no later step of the run would clear. A run with
the tool attached, and a session that writes no note, get the prompt as
built. The notice is added to the prompt sent to the agent only; the stored
first user message is unchanged, and the run logs a warning when it adds
it.

The `---` parser is kept only as a fallback. When the hooks receive no
submission they scan the turn's assistant messages as before, and log at warn
level (`no write_note call this turn; falling back to the "---" separator
parser`) when that fallback produces a note.

## Detached recovery

`recover_dead_sessions` (startup and the 60 s sweep) used to mark every
running session whose owner process died as `error` / `app_quit` and skip the
hooks. For a note session on a branch with a Blox workspace whose turn it
does not reattach to (see _A turn that outlives its connection_), it now
spawns a collection thread instead (guarded against overlapping sweeps
in-process):

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

### A turn that outlives its connection

Blox keeps the agent and its turn alive when the laptop side goes away, and
Staged follows such a turn to its end instead of failing it. Three paths do
that, and each one stores the note the agent published:

| What went away                                                                       | Who follows the turn                                           | Where the outbox is read                                                              |
| ------------------------------------------------------------------------------------ | -------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| The `sq blox acp` proxy; the run is alive                                            | The run itself, inside its turn loop (`pick_up_detached_turn`) | The loop's own collection, with the run's turn id                                     |
| The Staged process (killed or crashed), after the agent answered the turn's prompt   | The dead-owner sweep (`reattach_dead_owner_session`)           | `taken_over_note::reattached_turn_run_end`, any turn id                               |
| The run, leaving the row `disconnected`: a quit mid-turn, or a reattach that gave up | Reconcile (`Reconciler::run_completion_hooks`)                 | `taken_over_note::collect_taken_over_note` through `ReconcileBlox::exec`, any turn id |

The first path is a normal turn end (see _Collector_). Once its reattach
has returned and the run goes on, the run drops the mark that let a quit
detach it (`SessionRegistry::reattach_ended`), so a quit during a later
turn cancels the run as it does for any other run instead of counting it
as detached. The other two are
takeovers: the run that prompted the turn is gone, and with it the
submission slot and the turn id. They read the outbox once the turn has
completed and settle from what it holds (`TakenOverNote`):

- a published note whose Pikchr blocks parse goes to the post-completion
  hooks like the note of any completed turn, and the outbox is cleared once
  the hooks report it stored. A note they could not store stays there for
  the next run's recovery;
- a published note with a block that does not parse is not stored, because
  the repair loop went with the run. The row ends `error` / `detached` with
  the message a lost connection gets for such a note, without the polling
  marker, and the outbox is cleared so a Resume has the agent publish again;
- nothing published: the turn completes as it did before, through the `---`
  fallback or with an empty note, and the outbox is cleared;
- an outbox that could not be read is never taken for an empty one. The row
  ends `error` / `detached` with the polling marker and nothing is cleared,
  so the sweep keeps reading it (_Polling a detached row_).

A dead owner's reattached turn that failed is read as well
(`ReattachedTurnEnding::Failed`, `TakenOverNote::run_end_of_failed_turn`).
The case that showed it: Staged is killed after the agent published, and
relaunched while the turn is still running. On `claude-agent-acp` 0.100.0
the reattach's `session/load` interrupts the turn, the agent answers the
dead owner's prompt as cancelled, and the turn fails as `Remote agent lost:
the agent answered the turn's prompt as cancelled, so it did not finish`.
The row used to end `error` / `remote_agent_lost` with the published note
left in the outbox until a Resume. The agent has answered, so the turn is
over and one read is conclusive:

- a published note settles the row through
  `lost_remote_agent_terminal_state`, the rule for a run that loses its
  agent after `write_note`. With every Pikchr block parsing the session
  ends `completed` / `turn_complete` and the hooks store the note. With a
  broken block the row ends `error` / `detached` without the polling
  marker, the note is not stored, and the outbox is cleared;
- nothing published: the turn fails as before, `error` /
  `remote_agent_lost` with its own error, and the outbox is cleared;
- an outbox that could not be read: the turn fails as before, and the
  end-of-run clear removes only an unpublished outbox, so a published note
  waits for a Resume.

The failed ending reads the outbox instead of parking the row for the
detached sweep. The sweep's message says the agent may still be running
when it has in fact answered, and it would replace the `remote_agent_lost`
reason on a row that published nothing.

A reattached turn that was stopped, or that the agent cancelled on its
own, is not read. Its end-of-run clear follows the rule in _Collector_, so
it removes a published note only after a Stop. The Stop is looked for
twice: before the read, and again after it (`reattached_turn_run_end`'s
`stopped_since`), because the read is a round trip with the row still
`running`. A Stop the user presses meanwhile ends the turn as a stopped
one and what the read returned is dropped; it used to reach the terminal
state behind a settlement that outranks the cancel, so a failed turn's
valid note was stored and an unreadable outbox parked the row for the
sweep. A quit during the read is not a Stop and changes nothing. Why
`session/load`
interrupts a live turn on that bridge is not settled here, and neither is
the Quit row that stays `disconnected` after a mid-turn relaunch (reconcile
reads that turn as ended with background work still live).

All three paths used to lose the note. The proxy case reattached after the
turn loop, where the submission slot had already been read: the run
completed with an empty note and then cleared the outbox, deleting the only
copy. The two takeovers ran the hooks with no note at all, so the row
completed with an empty note and the published one sat in the outbox until
a Resume ingested it. Sessions that write no note are unaffected, since
they have no outbox.

The dead-owner sweep reattaches only when the agent answered the turn's
prompt (`dead_owner_recovery`). A note session on a Blox workspace that it
does not reattach to takes the claim-and-collect path at the top of this
section.

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
  window, an unreachable workstation, a known-bad note, a note the sweep
  turned down, a row written before polling existed) is never polled.
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
- **Checked before it is claimed or stored.** Both sweep paths run the
  Pikchr check on a found note first (`swept_note_pikchr_error`): the
  startup recovery of a dead owner's row before it calls the hooks, and the
  poll before its storing claim. The sweep was the one reader that did not
  check, so a parked row whose agent had published a broken diagram was
  completed with that note within a minute. No run is left to ask for a
  repair, so a note with a block that does not parse is not stored. The row
  stays `error` / `detached` and gets `detached_note_rejected_message`
  through the same guarded write: "…its note "<title>" is complete on the
  workstation, but the Pikchr block at line N does not parse (…), so the
  note was not stored. Resume to have the agent fix the diagram in its
  draft." The message has no marker tail, so the row is not polled again
  and the chat pane shows it as written. The outbox and the draft are left
  on the workstation, because the Resume is the repair: its start-of-run
  recovery finds the note, keeps the draft and opens the run with the
  correction (_A waiting note with a broken diagram_).
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
the session."; or, for a turn that ended with its outbox unread, "The turn
of this remote session ended, but Staged could not read its note from the
workstation. Staged keeps checking and will complete the session when it
finds the note."). Once both are gone the row drops the "yet", and the chat
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
  `hold_stopped_note_unstored_message` and the outbox for a Resume. A late
  note with a Pikchr block that does not parse is not claimed or stored
  either: the row gets `hold_stopped_note_rejected_message`, which settles
  it, and the outbox and the draft stay for a Resume to repair, also past
  the window. The row is `completed`, so the chat pane does not show that
  message: the note stays empty, the hold-stopped copy and its Resume
  button stay, and the reason is in the log and in the agent's first prompt
  after the Resume. This is the one place an `error_message` is written on a
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
  pane's note card makes that moment visible. The moment is short now. The
  turn ends with the "Done" message about 2 s after the call (2.3 s
  measured on Fable, 1.4 s on Opus and Sonnet), where the recap took 4.5 s
  in the same runs and 7 s in stored sessions.
- **A saved turn ends with a hidden "Done", not a hard stop.** Neither MCP
  nor ACP lets a tool end the turn, so a real stop needs a mechanism per
  agent: a Claude `PostToolUse` hook, a `session/cancel` for Goose, nothing
  for Codex, and driver changes so a hook-stopped turn still finishes.
  Asking for one word needs none of that, costs one short model call per
  saved turn, and works for every agent. In live runs the text after the
  last saved call was exactly `Done` in 146 of 146 Claude turns with no
  user question, and in 4 of 4 turns each on Goose and Codex. It also keeps
  a reply where the user asked for one, which a hard stop would cut. The
  hook stays in reserve for Claude. Add it if the drift log shows the recap
  coming back, or if the 2 s and the Stop window above prove to matter.
- **The "Done" is hidden when the pane renders, and the row is stored.**
  The other choice was to drop the row in the message writer. A wrong match
  there loses the text, where a later release can show a hidden row again.
  The writer also sees only a call's raw status, and a remote call
  completed from the outbox never gets `completed` there, so only the
  transcript builder knows that call was saved. The cost is that backend
  comparisons count the row (_Ending a saved turn: the hidden "Done"_).
  Making them exact needs the dropped row or a per-row marker, which is a
  data-model change.
- **A `Done` line at the end of a real answer is shown.** Four turns in the
  live runs ended an answer with a last line `Done`. Stripping that line
  would hide it at the cost of editing text the agent wrote, so the pane
  shows the message whole and the drift log marks it in its line.
- **The hiding rule is keyed on the call, not on the text.** The old `---`
  scan was a text test applied to every assistant message, and it had to
  agree with a backend parser by convention. When delivery moved to the
  tool the scan matched nothing and nothing failed, so the note card
  disappeared unnoticed. The text test for "Done" runs only directly after
  a completed `write_note` call, it fails open, and the word has one source
  that tests on both sides read.
- **The note card replaces the saved call's row.** With "Done" hidden a
  saved turn ends at the call. A card at the end of the turn would sit
  directly under the "Saved note" row and repeat it, so the completed call
  is the card. The final-turn rule keeps the row for a call that did not
  change the stored note, so the card never claims an update that did not
  happen and the "Ask for the note to be updated" button has a row to sit
  under.
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
- **A takeover's prompt answered as cancelled is read as the reattach
  interrupting the turn, and its note is stored.** When a dead owner's turn
  fails because the agent answered the owner's prompt as cancelled
  (`ReattachedTurnEnding::Failed`), Staged takes the cause to be the
  reattach's own `session/load`, which interrupts a live turn on some
  bridges, and stores a valid published note. A Stop the dead owner sent
  just before it died produces the same answer, and the two cannot be told
  apart from here: the Stop went with the process that sent it, and the
  agent says only "cancelled". So a note published before such a Stop is
  stored, against the rule above. The window is the cancel's acknowledgement
  time (0.6 s measured on Blox) followed by the owner dying before it
  records the Stop, and the other reading would lose the note of every
  mid-turn relaunch, which is the common case. A Stop pressed in the
  instance that took the turn over is a different matter and does win, up
  to and including the takeover's outbox read.
- **A Stop pressed during a late outbox read is still a Stop.** Two reads
  happen after the turn is over with the row still `running`: the second
  read of a completed run whose first read failed, and the read on a
  taken-over turn's ending. Both look for the Stop again after the read and
  end the run `cancelled` with the outbox discarded, instead of settling
  from the read ahead of the cancel. A quit is not a Stop and does not
  discard anything.
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
- **The file is the note; the call only publishes it.** `write_note` takes
  no body, not even an optional one. An optional body would keep the
  re-emission cost for every agent that chose it and leave two ways for the
  body to arrive, and mirroring an inline body into the file would make the
  file a copy rather than the note. Publishing stays an
  explicit call rather than "whatever the file holds when the turn ends":
  a half-written draft must not become the note because a turn ended.
- **The title is the draft's first line, not an argument.** This reverses
  an earlier decision recorded here. The title used to stay an argument,
  required in both schemas, so that the body never needed parsing for it
  and an H1 in the draft stayed body text. The seed followed the same rule
  and wrote the stored body verbatim, with no title in the file.

  It was reversed for amendments and for one shape. An amending agent could
  not see the stored title in its draft and retyped it in the call, which
  could drift and overwrote the ` (2)` suffix a move adds. With the title
  line, the draft is the text the note viewer renders, Copy produces, the
  `---` fallback accepts and the written-note editor already uses. The call
  shrinks to one optional argument.

  The earlier entry also recorded a bug: the first version of the seed
  stripped a leading H1 from the stored body, for older notes that might
  carry a title line, and silently dropped a real `# Section` heading on
  the next amendment. That pair was not symmetric. The seed removed a line
  that the save had never added, so a heading that belonged to the body was
  taken for a title. The new pair is symmetric. The store holds the body
  without the title, the seed always adds exactly one line (`# {title}`,
  above a blank line) and the save always removes exactly one, the first.
  Neither looks at what the body opens with, so a body that opens with
  `# Section` has that heading on the draft's third line and gets it back
  (`a_body_that_opens_with_a_heading_survives_seed_republish_and_store`).

  What is stored did not change, so this is a change at the two handlers
  and nowhere downstream: the title and the body stay separate columns, the
  outbox keeps its files and keys, and there is no migration. Existing notes
  need none either, since a stored body never held the title.

  The rule is strict and one line long. Only `# ` plus text on the first
  line is a title: a `##` heading is not read as one, because stored bodies
  do open with `##` sections, and a closing `#` is kept as text so that the
  two parsers stay trivial. The cost is that an invariant the schema used to
  enforce is now enforced by the prompt and a rejection. A missing `title`
  argument was rare because the schema required it; a missing title line
  costs one rejected call and a one-line edit. How often a first call is
  rejected for it has not been measured in live runs, the way the earlier
  wording changes in this document were. The rule also exists twice, in
  `note_draft.rs` and in `staged-mcp.cjs`, and a change to it must land in
  both; `write_note_reads_the_same_drafts_on_both_transports` is the guard.

- **A stale `title` argument is compared, not ignored, and only for now.**
  A session resumed with the earlier instructions in its context may still
  pass `title`. Ignoring it would silently drop a rename; rejecting every
  such call would cost an extra call even when the argument says what the
  draft says. So it is accepted when it agrees with the title line and
  rejected with a text that names the line when it does not (_The title
  line_). This is temporary. The tolerance is to be removed from both
  handlers once sessions started under the earlier contract are unlikely to
  be resumed, and that removal has not been done.
- **The collector reads the script's version to take the title.** A title
  the script parsed from the draft must be stored as written, and a title
  that was an argument of an earlier script must still lose one leading `#`
  (_The title depends on the script's version_). The version was already
  in `meta.json`, so no key was added and the outbox format did not change.
  Two app builds that share a workstation can still pair one build's prompt
  with the other's script. That is limited to development and is not
  handled.
- **A fresh note gets no file; an amendment gets the stored note.** An empty
  pre-created file forces a Read before the first Write in Claude Code and
  tells the agent nothing. A seeded file is what makes an amendment an edit:
  without it the agent would have to re-emit the note from chat history,
  which is the cost the draft removes.
- **Clear policy.** The draft follows the outbox: removed locally at run
  end, removed on the workstation by the same command that clears the
  outbox, and kept wherever the outbox is kept. The outbox is cleared at the
  end of every run that read it after its last turn, and after a Stop. A run
  that ended without that read removes only an unpublished outbox, so a
  published note is never deleted unread. It is kept outright when the sweep
  is still polling for the note, when the start-of-run recovery did not
  settle and the run never collected its own note, and when a collected note
  could not be stored (above). The sweep clears it again after it stores a
  late note, and after a hold-stopped row's window expires. Clearing at
  start was rejected because it would delete an uncollected note on every
  resume; never clearing was rejected because a stored note would be
  re-ingested on the next run, over whatever the user edited meanwhile.
  Clearing at the end whatever the run had read was what the first version
  did: a run that reattached after its proxy died stored an empty note and
  then deleted the published one.
- **A broken diagram found without a run is repaired by a Resume, not
  stored and not deleted.** The sweep and the start-of-run recovery read a
  published note when no run is left to ask for a repair. Storing it anyway
  (what the sweep did) put a note with an unrendered diagram in the store
  with nobody told. Deleting it (what the recovery did) cost the whole note:
  the agent rewrote it from memory. So the sweep parks the row and keeps the
  outbox and the draft, and the Resume's recovery turns the note into a
  repair request against the kept draft. The two changes only work as a
  pair. The check in the sweep alone would have sent the user to a Resume
  that deleted the note. A run that loses its agent, or takes over a turn,
  and then finds a broken note still clears it and asks for the note to be
  written again; that path was not changed.
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

1. Start a note session on a local branch (or a project note). The log shows
   `note draft <temp dir>/staged-note-draft-<session-id>.md absent for a
fresh note`. The agent writes that file with its file tool, the title as a
   `# ` heading on the first line, then calls `write_note` with the next
   steps only. In the chat pane the call shows as a "Saving note" row while
   it runs, the verb alone with no title beside it, with the suggested next
   steps in its details and no body preview. When the call completes the row
   becomes the "Note created" card. The stored note's title is the text of
   the draft's first line, and the note viewer shows that title once. Nothing shows under the card: the agent's closing "Done" is
   hidden, and the log has no `note-done drift` line for the session. The
   note row updates when the turn ends. After the run the draft file is
   gone.
2. Check the log for `note delivered via write_note (Tool)`. If you see the
   warn line about the `---` fallback, the agent ignored the tool: check the
   prompt reached it (`note_delivery_guidance` text, which names the draft
   file) and that the `staged` server appears in the ACP `session/new`
   request.
3. Put an invalid Pikchr block in the draft: the correction prompt must name
   the draft file and the block's line (a line of the note body, counted
   from the first non-blank line after the title line), the agent must edit
   that block in the file (an Edit, not a rewrite) and call `write_note`
   again with the next steps only, and the stored note must be the repaired
   one. (Applies to remote sessions
   too: the outbox is collected after every settled turn, so the same
   correction prompt and re-collection happen there.)
4. On a fresh note, ask the agent (in the prompt) to not call any tools:
   after the turn, one follow-up user message asking it to write or finish
   the draft and call `write_note` must appear, and the note must be stored
   after the second turn. Then, on that stored note, ask a chat question
   that needs no note change ("no need to change the note: …"): no
   follow-up message must appear, the note's content and timestamp must be
   unchanged, and the chat pane must offer "Ask for the note to be updated".
5. Local Claude amendment: on a Claude note session whose note is stored and
   holds an embedded image, send "Please update the note: add a short
   Risks section". The log shows `note draft … seeded with the stored note
for amendment`, and before the agent's first tool call the file holds
   `# <stored title>`, a blank line and the stored body verbatim, with the
   `staged-media://` reference unchanged
   (`cat "$TMPDIR"/staged-note-draft-<session-id>.md`). The transcript must
   show a Read of the draft, an Edit that adds the section, and a
   `write_note` call; it must not show a Write of the whole note. The stored
   note keeps its title and its image, and the draft is gone after the run.
6. Codex fresh note in `$TMPDIR`: start a note session with Codex as the
   provider on a local branch. Codex must create
   `$TMPDIR/staged-note-draft-<session-id>.md` without a sandbox or
   permission prompt (`echo "$TMPDIR"` in a terminal gives the directory
   Staged's `std::env::temp_dir()` resolves to), call `write_note`, and the
   note must be stored. Then call `write_note` by hand against a missing
   draft to see the rejection: ask the agent, in a new session, to call
   `write_note` before writing anything. The tool result must read
   `write_note rejected: the draft file … does not exist …` with the
   absolute path, and the agent must recover within the turn. The rejected
   call keeps its "Note rejected" row, and the retry below it is the note
   card.
7. The card and the button. On a stored note, ask for a change ("add a
   short Risks section"): the new call shows as a "Note updated" card, the
   first one still reads "Note created", and no "Ask for the note to be
   updated" button shows. Then ask the agent to publish the note again
   without changing it ("call `write_note` again with the draft as it is,
   and the same next steps"). After the turn settles that call
   shows as a "Saved note" row, not a card, the note's timestamp is
   unchanged, and the button shows under the row.
8. A question in a note turn. On a stored note, ask for a change and a
   chat answer in one message ("add the file name to the note, and tell me
   here which line holds the bug"). The answer must be visible, above the
   card or under it, and no bare "Done" may show. An answer under the card
   is logged as drift, which is expected for this turn.
9. Drift. Ask the agent to "write a three-sentence summary in chat after
   you save the note". The summary shows under the card, and the log holds
   one line for the session that contains `note-done drift` and ends with
   the count and the length, for example
   `1 message after the saved write_note call, the last 412 chars`. A turn
   that ends in the hidden "Done" logs no such line.
10. A real agent. From the repository root run
    `cargo test -p acp-client real_bridge_saved_note_turn_ends_with_a_bare_done -- --ignored --nocapture --test-threads=1`.
    It needs the bridge installed, a signed-in CLI, network and `node`. It
    attaches the shipped `staged-mcp.cjs` with a temporary home directory,
    asks for a short note without saying how to end the turn, and asserts
    that every row after the saved call is a bare "Done". Set
    `ACP_REAL_BRIDGE=codex` or `ACP_REAL_BRIDGE=goose` for the other
    bridges (Goose under a headless runner also needs
    `GOOSE_DISABLE_KEYRING=1`). Run it for each bridge when the drift count
    rises or a new model ships.
11. The title line. In a new note session, ask the agent to start its draft
    with a plain sentence and no heading, then call `write_note`. The tool
    result must read `write_note rejected: the draft file … does not start
with the note's title. …`, the agent must add the `# ` line and call
    again, and the chat pane shows a "Note rejected" row and then the note
    card. Then, on that stored note, ask for a rename only ("rename the note
    to …"): the agent must edit the draft's first line and call `write_note`
    with the next steps only, the timeline row shows the new title, and the
    note body is unchanged.
12. First calls rejected for a missing title line. Run fresh and amending
    note sessions on Claude, Goose and Codex and count the first
    `write_note` calls that are rejected with the text of step 11 when
    nothing asked for it. This count has not been taken yet. If it is more
    than a few percent, the wording of `note_delivery_guidance` needs
    tuning.

Blox:

1. Check the Staged MCP on Blox first (`node`, install, the attach log line):
   see _How to test manually_ in [`staged-mcp-on-blox.md`](./staged-mcp-on-blox.md).
2. Start a note session on a remote branch. The prompt names
   `/home/<user>/.staged/sessions/<session-id>/draft.md` with the real home
   (no `~`, no `$HOME`), and the log line `Staged MCP on Blox attached …
note draft at <path>` names the same file. After the `write_note` call,
   `~/.staged/sessions/<session-id>/` on the workstation holds `draft.md`,
   `note.md` (the draft without its title line, trimmed), `meta.json` (its
   `title` is the text of that line and its `helperVersion` is 10) and `done`.
   `done` holds 32 hex characters, and a second `write_note` call changes
   them. The commands before the agent starts are the ones that ran before the draft
   existed (the install probe, the outbox read, the clear); none was added
   for the draft. The chat pane shows the "Note created" card with nothing
   under it, as it does locally: the result text and the hidden "Done" are
   the same on both transports.
   Goose amendment on Blox: on a Goose remote note session whose note is
   stored, send a follow-up asking for one added paragraph. Before the
   agent's first tool call, `cat ~/.staged/sessions/<session-id>/draft.md`
   on the workstation shows `# <stored title>`, a blank line and the stored
   body verbatim (`staged-media://` references included); no extra `ws exec`
   was spent on it (the clear
   command carries the base64 seed). Goose must open the file by the
   absolute path from its prompt, edit it in place, and call
   `staged: write note` with the next steps only. The stored note has the
   same title, the new paragraph and every earlier section byte-for-byte.
3. Disconnect tests. Use a prompt that keeps the turn open after the note
   is published, and wait for the note card ("Note created") before each
   kill. A bare
   `sleep 120` does not do it on Claude: its harness blocks the long sleep,
   so the turn does not stay open. A shell loop of short sleeps works, for
   example "write the note, call `write_note`, then run
   `for i in $(seq 1 24); do sleep 5; done`".
   Proxy kill: run `pkill -9 -f 'acp <workspace-name>'` on the laptop. The
   log shows `Remote agent detached: … reattaching to its turn`, the row
   stays `running`, and when the turn ends the log shows `collected note
"<title>" from the workstation outbox`. The note row fills in with the
   published body and `~/.staged/sessions/<session-id>/` is gone afterwards.
   The log must not show `storing an empty note`.
   App kill: `kill -9` the Staged process instead and relaunch. When the
   relaunch is, relative to the end of the turn, decides which path runs,
   so try both. Relaunched after the turn has ended on the workstation
   (wait out the sleep loop): the sweep reattaches, the log shows `the
reattached turn has ended` and then `collected note "<title>" from the
workstation outbox after its turn ended on a takeover`. Relaunched while
   the turn is still running (within a few seconds of the kill), on a
   workstation whose bridge is `claude-agent-acp` 0.100.0: the reattach's
   `session/load` interrupts the turn, the log shows `Remote agent lost:
the agent answered the turn's prompt as cancelled, so it did not finish`,
   then the same takeover collection line and `lost the remote agent after
the note was delivered … completing from the workstation outbox`. In both
   cases the row ends `completed`, the note row fills in with no Resume and
   the session directory is gone. A bridge that does not interrupt the turn
   streams the rest of it and ends as the late relaunch does.
   Quit: quit Staged mid-turn and relaunch after the turn has ended on the
   workstation. The row is `disconnected` until reconcile finds the turn
   over, then the same takeover log line appears and the note row fills in.
   Relaunching while the turn is still running does not give that result
   on bridge 0.100.0: the row stays `disconnected` with the note kept in
   the outbox, and Stop or a later Resume stores it. That is a known open
   item, not a result to expect from this test.
   Detached sweep: this path is reached when the connection to the agent is
   lost rather than the proxy (the hung-proxy test below), or when Staged
   dies after sending the prompt and before the agent's first output. For
   the second, start a remote note session, `kill -9` Staged about a second
   after the prompt is sent, and relaunch once the agent has had time to
   write its note. Do not kill as soon as the row shows `running`: that is
   before any command has reached the workstation, so no agent was started,
   and the row then polls for a note that cannot arrive (the relaunch logs
   `no completed note in the workstation outbox yet` and nothing follows).
   The startup sweep logs `Recovered detached remote session … note "<title>" found` and
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
   Broken diagram: have the agent publish a note with a `pikchr` block that
   does not parse and reach the detached sweep as above (or write such a
   note by hand, step 5, into the outbox of a detached row, with a
   `draft.md` beside it). The next sweep tick logs `has a Pikchr block at
line N that does not parse … not storing it`, the note row stays empty,
   and the chat pane shows "…its note "<title>" is complete on the
   workstation, but the Pikchr block at line N does not parse (…), so the
   note was not stored. Resume to have the agent fix the diagram in its
   draft." with a Resume button. On the workstation `done`, `meta.json`,
   `note.md` and `draft.md` are all still there. Press Resume: the log shows
   `not storing it, keeping its draft for this run to repair` and `asking
the agent to repair the Pikchr block at line N`, the outbox files are
   gone while `draft.md` is unchanged, the agent edits the block in the
   draft and calls `write_note`, and the note row fills in with the repaired
   diagram. The chat shows only the resume message the user sent, not the
   appended correction. This check has not been run against a workstation.
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
   `mkdir -p ~/.staged/sessions/<session-id> && printf '# Hand\n\nWritten by hand\n' >
~/.staged/sessions/<session-id>/draft.md`, then
   `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{}}}' | node
~/.staged/bin/staged-mcp.cjs <session-id> <turn-id>`. The result reads
   `Saved note "Hand" (15 bytes from the draft file). …`, `meta.json` holds
   `"title":"Hand"` and `"helperVersion":10`, and `note.md` holds
   `Written by hand` with no heading. Without the draft the same call
   answers `isError: true` naming the missing file and writes nothing into
   the outbox. So does a draft with no title line
   (`printf 'Written by hand\n'`), with the "does not start with the note's
   title" text. For the stale argument, repeat the call on the `# Hand`
   draft with `"arguments":{"title":"Hand"}` and then
   `"arguments":{"title":"# Hand"}`: both save the note as "Hand". With
   `"arguments":{"title":"Other"}` the call answers `isError: true` with
   the "`title` is no longer an argument" text, which quotes "Hand", and
   the outbox keeps the earlier call's files.

Automated coverage: `note_draft::tests` (paths, each bad-draft case and its
error text, one leading byte order mark dropped before the trim, a symlink
followed, a FIFO rejected without blocking, an unreadable file,
`split_title` over every row of the table in _The title line_, CRLF and
CR-only line endings, U+2028 and its kind kept in the title, and the two
title error texts, the seed with the title line above
the stored body and a leading heading and media references kept, the
heading-safe seed title changing line breaks only, every title the split
returns seeded as written, a seeded draft published untouched giving the
stored title and body back for titles with tabs, runs of spaces, no-break
and ideographic spaces and U+2028, seeding and removal, the prompt path, the
unresolved-home placeholder and the home epilogue), `note_tool::tests`
(schema with `suggested_next_steps` only, no `title`, no `body` and nothing
required, the tool texts putting the title in the draft and not in the
call, the `initialize` instructions naming the draft, the handler
publishing and republishing the draft with the title from its first line,
each bad draft rejected by path without touching the slot, a draft with no
title line or only a title line rejected the same way, a stale `title`
accepted when it agrees with the title line and rejected when it does not,
and `from_parts` beside `from_parts_as_written` for the two outbox title
rules, a `\r` in the title stored by both),
`session_runner::tests::note_draft` (a fresh local run removes a stale draft
and creates none, an amending run seeds it with the title line and the body
for branch and project notes and with the same text on both transports, a
body that opens with `# Section` survives seed, untouched republish and
store twice over with its timestamp unchanged, and so does a title with
tabs, runs of spaces, no-break or ideographic spaces or U+2028 in it, an
edit of the title line or
of the body changes what is stored, a stored title that is not one line
seeds a draft that publishes and is left alone by the next seed, the
draft is removed at run end, a remote run start costs the same two commands
with or without a seed, a failed seed leaves the recovery settled and so
does a rejected waiting note, only a rejected recovery carries a note to
the first prompt, a run whose recovery rejected a note opens with the
prompt as built, a notice and the unchanged submitted-note correction for
this run's draft, the
follow-up and Pikchr repair prompts name the
draft, an error found in the reply makes no claim about the draft, a run
without the tool is pointed at the `---` format and never asked to call
`write_note`, a published note is validated instead of the unsaved reply
while a run with no tool or no published note still has its reply checked,
a remote repair turn is judged by the re-collected note and not by a broken
block quoted in its reply, a note run without the tool gets the notice
in its first prompt, locally and on a workstation, while a run with the tool
gets none, and the notice and the no-tool follow-up say "in this run" and
show a `suggested-next-steps` block that `extract_suggested_next_steps`
parses, both alone and when the whole prompt is fed to it),
`session_commands::tests` (the `$HOME` placeholder caution
appears with an unresolved home and only then),
`remote_staged_mcp::tests` (the Staged MCP on Blox itself: install,
injection, protocol; see [`staged-mcp-on-blox.md`](./staged-mcp-on-blox.md)),
`remote_staged_mcp::note_outbox::tests` (outbox parsing including a
marker that changed between the reads and a marker whose token changed
between them, with equal, differing, empty and half-empty tokens,
`outbox_state` keeping a version 8 title as written and normalizing the
title of an earlier or unversioned outbox, and storing a title with a
`\r` at every version, through the collect script and the run-start
recovery too, the
collect script printing the token on both marker lines under a local `sh`,
a whole rewrite made to land between the script's two file reads never
collected as one note, two `write_note` calls writing different tokens,
transport vs content failures,
per-turn collection into the slot, start-of-run recovery that discards
unusable content, and that rejects a note with a broken Pikchr block
without storing it, removing only its publication and keeping the draft
(recorded commands, a removal that cannot be confirmed, and a local `sh`
run with the draft as published, edited since, and missing), the reject
script writing a draft that the handler and the script then accept,
`write_note` run as a subprocess with fake JSON-RPC
including a rewrite blocked between its renames and a call with no
`draft.md`, the draft's heading published as the `meta.json` title with no
heading in `note.md`, a draft with no heading or only a title line rejected
with the outbox untouched, a stale `title` argument that must agree with
the heading, an outbox written by a version 7 script still collected by
turn, by the sweep and by the run-start recovery, the clear removing `draft.md`, the seed riding the recovery's
clear command and a seed too large for one command, a failed seed reported
apart from the settled recovery, every clear removing `.draft.md.seed`, a
draft left by a lost run never collected or ingested without a call, local
`sh` run of the
collect, clear, seed-in-clear and recovery scripts, schema parity with the
Rust tool, and `write_note_reads_the_same_drafts_on_both_transports`, which
runs the laptop's handler and the script over the same draft
files — missing, blank, byte order mark cases, Unicode whitespace, not
UTF-8, over and at the cap, a directory, a FIFO, a symlink, an unreadable
file, drafts with a title line, with none and with only a title line,
drafts with CR-only and mixed line endings, a table of stale `title` calls,
and seeded drafts published untouched — and requires the same result text,
title and published body), `session_runner::tests::detached_recovery` (claim-first
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
frontend marker parity for both suffixes, the Stop marker and the
unread-outbox marker),
`session_runner::tests::swept_note_pikchr` (neither sweep path stores a
note with a broken diagram: the startup recovery and the poll leave the row
`detached` with the rejected message and no marker, a stale storing claim
is checked too, a resumed row is not overwritten, a hold-stopped row is
settled in place and keeps its outbox inside and past the window; and,
against a real shell, the sweep leaves the outbox and the draft, the
Resume's recovery keeps the draft and opens the run with the correction,
and the note republished from the repaired draft is the one stored),
`remote_staged_mcp::note_outbox::tests` (a probe reports the note already
collected as pending until its stamp or content changes; `FileCksum`
agrees with the `cksum` command; against a real shell the guarded clear
removes the note whose stamp it is given and keeps a republished one),
`session_runner::tests::unread_run_end` (a rejected note is cleared at an
unread ending and the next run's recovery finds nothing, while a
republished or never-rejected note stays; a completed run whose last read
failed is parked for the sweep with the turn-ended wording, which then
stores the note, or settles from a second read that went through; a valid
note the hooks never ran for is kept; a still-broken note a repair turn
republished survives the guarded clear and is rejected, not stored, by
the next run's recovery, with its draft kept; a Stop during the second read ends the run as
stopped whatever the read returned, and a quit there does not),
`session_runner::tests::taken_over_note` (the takeover endings,
including a dead owner's turn answered as cancelled: a valid published
note completes the session, a broken one is rejected and cleared, and no
note or an unreadable outbox fails as before; a Stop during the takeover's
read ends the turn as stopped for every ending and outbox state, and a
quit there does not),
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
follow-up paths (`session_commands::tests`: every note prompt names the
draft once with the title-line and amendment wording and no `title` or
`body` argument; the grammar upload
returns the workstation home from its last command), and on the frontend
`noteToolCall.test.ts` (next steps with no title, a call with no arguments,
a call recorded with a title, no body, the row detail and the
has-details check, and the tool name in
each bridge's spelling from the shared case table),
`NoteToolDetails.component.test.ts` (steps with no title line, a recorded
title above the steps, no panel for a call with neither, and no body
preview or "No note body"
row), `toolCallViewModel.test.ts` /
`acpTranscript.test.ts` (the `write_note` row for all three bridges, the
verb alone for a call with no title, a call with no arguments that expands
only onto its result, rows named after a recorded title,
interrupted pending calls, and the grouping predicate that keeps note cards
out of a collapsed group), `sessionModalHelpers.test.ts` (the detached, storing and unread-outbox copy and which
end-of-session notice the chat pane renders), `acpTranscript.test.ts` (only
the final turn's unsettled calls are relabelled in a completed session) and
`sessionFailureCopy.test.ts` (the timeline copy says "yet" only while
`isPollingForNote` is set).

For the ending of a saved turn: `note_tool::tests` and
`session_commands::tests` pin the result text, the description's last
sentences and the last paragraph of the guidance in every note prompt.
`saved_result_text_is_identical_for_local_and_remote_sessions`
(`remote_staged_mcp::note_outbox::tests`) gives the local handler and the
script the same draft and compares the two result texts.
`noteDoneWording.test.ts` reads `note_tool.rs`, `staged-mcp.cjs` and
`session_commands.rs` as text: each result text, each description and the
session prompt must quote exactly one closing word, and it must be the
`NOTE_DONE_WORD` that `isNoteDoneText` accepts. `noteDoneMessage.test.ts`
runs `isNoteDoneText` over the shared case table and
`hiddenNoteDoneMessageIds` over groups built by `buildAcpTranscriptGroups`:
a match in each bridge's spelling, a rejected, running and interrupted
call, a tool call after the text, more text after "Done", a reply between
the call and "Done", an answer ending in a `Done` line, two "Done"
messages, Codex's extra item, a remote call completed from the outbox,
several turns, and another server's `write_note`.
`SessionChatPane.noteDone.component.test.ts` mounts the real pane and
asserts the message is absent for each bridge, present after a rejected or
interrupted call, and back when more text or a tool call follows it.

For the note card: `noteDeliveries.test.ts` (each bridge's spelling, calls
that are not deliveries, a remote call completed from the outbox, a
rejected call and its retry, the created and updated labels, the separator
shape alone and mixed with calls, and a table for the final-turn rule
checked against `shouldAskForNoteUpdate`) and
`SessionChatPane.noteCard.component.test.ts` (the real pane, card and tool
row: the card for each bridge, the two labels, a reply under the card, the
kept row for a rejected and an interrupted call, two cards in a row, a
running call turning into the card, the text shape, and with a note context
the card without the button, the row with the button, the swap when the
note refreshes, and a running session that settles with the note
unchanged).

For the follow-up rule and the drift log: `session_runner::tests::note_draft`
(a table for `note_write_followup_due`, and a chat-only turn on fresh and
amended branch and project notes) and `note_done::tests` (both rules over
the shared case table, the drift decision for each bridge's spelling, every
accepted and rejected text, a recap, a turn that ends at the call, an empty
slot, a turn with no call, earlier turns, a steered message, other tool
rows beside the call, a second "Done", a recap after "Done", an answer
ending in a `Done` line, a tool call after the text, two calls in one turn,
the wording of the log line, and three store-backed tests with rows the
real message writer stored). The ignored
`real_bridge_saved_note_turn_ends_with_a_bare_done` in `crates/acp-client`
holds a real agent to the same rules: it compiles `note_done/rules.rs` by
`#[path]`.
