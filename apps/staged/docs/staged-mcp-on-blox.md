# Staged MCP on Blox

The Staged MCP on Blox is Staged's MCP server on a Blox workstation. A remote
agent cannot reach the MCP servers Staged runs on the laptop's loopback, and
the laptop may disconnect while the agent is still working. So Staged uploads
a small stdio MCP server to the workstation and injects it into the remote
agent's session under the name `staged`. Its tools write what they produce to
the workstation, and the laptop reads that back over `ws exec` after the turn,
or later if it was disconnected. What a tool writes outlives the connection.

It serves one tool today, `write_note`, which delivers a note session's note
(see [`note-delivery.md`](./note-delivery.md) for that contract). Other tools,
for example Pikchr checks or progress reporting, are meant to be added to the
same server rather than as separate helpers.

## Where things live

| What                       | Where                                                     |
| -------------------------- | --------------------------------------------------------- |
| Server script (source)     | `src-tauri/resources/remote/staged-mcp.cjs`               |
| Install, injection, runner | `src-tauri/src/remote_staged_mcp.rs`                      |
| `write_note` outbox reader | `src-tauri/src/remote_staged_mcp/note_outbox.rs`          |
| Test helpers               | `src-tauri/src/remote_staged_mcp/test_support.rs`         |
| Installed script           | `~/.staged/bin/staged-mcp.cjs` on the workstation         |
| Installed checksum         | `~/.staged/bin/staged-mcp.sha256` (SHA-256 of the script) |
| Per-session scratch area   | `~/.staged/sessions/<session-id>/`                        |

The script is embedded in the build (`remote_staged_mcp::SCRIPT_SOURCE`,
`include_str!`), so a running Staged always installs the version it was built
with.

## Runtime

The script uses only Node's standard library. It is CommonJS (the `.cjs`
extension keeps it CommonJS whatever `package.json` is nearby, and
`apps/staged/package.json` is `"type": "module"`) and uses nothing newer than
Node 18 syntax. `node` was verified on a Blox workstation (`sq blox ws exec
<ws> -- node --version`, v26); `python3` is present too but unused.

A workstation without `node` fails the install. The session then runs without
the server and falls back to whatever the session type does without it. For a
note session that is the `---` contract.

## Install

At the start of a remote session that uses the server,
`remote_staged_mcp::install`:

1. runs one `ws exec sh -c` probe that creates `~/.staged/bin` (mode 700),
   removes the legacy files (below), and prints the home directory, the
   recorded checksum and the path of `node`;
2. uploads the embedded script through the base64 chunked
   `write_bytes_to_remote_with` path (temp file + rename) only when the
   checksum in `~/.staged/bin/staged-mcp.sha256` differs from the embedded
   script's, then makes it owner-only and records the new checksum.

An unchanged script therefore costs one round trip per session. The log says
`Installed the Staged MCP on Blox at <path> (<checksum>)` when it uploaded.

**Legacy files.** Builds from before the rename installed the same server as
the note-only `~/.staged/bin/staged-note-mcp.cjs` with
`staged-note-mcp.sha256`. The probe deletes both on every install, at no extra
round trip, so a workstation never keeps the old copy. An older build sharing
the workstation re-uploads its own copy on its next session, because its
checksum file is gone too. A session that build starts within the same few
seconds as a newer build's install can lose its copy and run on its fallback.

## Injection

`remote_staged_mcp::stdio_mcp_server` builds the `session/new` /
`session/load` entry:

```text
McpServer::Stdio { name: "staged", command: <node>, args: [<script>, <session id>, <turn id>] }
```

The name is `remote_staged_mcp::SERVER_NAME`. It is the local note server's
name too (`note_tool::NOTE_MCP_SERVER_NAME`), so a tool reads as
`mcp__staged__<tool>` in Claude Code whichever transport serves it. The turn
id is a per-run id the session runner generates. The server hands it to every
tool so the laptop can tell this run's output from an earlier run's.

Today only remote note sessions attach the server (`session_runner`, the
`is_note_session` branch of the run setup). A tool meant for other session
types widens that condition.

## Protocol

Newline-delimited JSON-RPC 2.0 on stdin/stdout, the MCP stdio transport:

- `initialize` answers with the client's protocol version when the server
  implements it (`2025-06-18`, `2025-03-26`, `2024-11-05`), otherwise with
  the newest one it speaks. It never echoes a version it does not implement.
  The result carries `capabilities: { tools: {} }`, `serverInfo` with the
  name `staged` and, as `version`, the script's `SERVER_VERSION` constant
  as a string, and `instructions` built from each tool's `instructions`
  sentence, in `TOOLS` order.
- `ping` answers `{}`.
- `tools/list` lists `TOOLS` in order. `tools/call` runs the named tool and
  returns its text with `isError`. An unknown tool is error `-32602`. A tool
  that throws or rejects is reported to the agent as an `isError` result (its
  `failureText`, or `<name> failed: <message>`) and logged, never a crash
  mid-turn; so is a tool that returns or resolves to something other than
  `{ text, isError }`. A `failureText` that itself throws is logged and the
  default text is used, so a failing call always gets a reply. A tool that
  returns a promise is answered when it settles, and requests that arrive
  meanwhile are served first, so replies can leave out of order (JSON-RPC
  matches them by id).
- `resources/list` and `prompts/list` answer empty lists. Any other method
  is `-32601`. A line that is not JSON is `-32700` and a non-object is
  `-32600`. Notifications and responses are ignored.
- Anything that still throws out of the request handler (a tool whose
  property getter throws during `tools/list`, say) answers `-32603` for the
  request's id, or is only logged when the line carried no id. The rest of
  the stdin chunk is served either way; an exception that escaped the
  readline listener used to make it drop the lines buffered behind the bad
  one.

The process ends when stdin closes: the server exits 0 as soon as every
pending `tools/call` promise has settled and the replies have been flushed.
A tool may hold a live handle (a child process, an interval) and still not
keep the process alive after the agent has gone. For `write_note`, which is
synchronous, nothing is ever pending and the exit is immediate. Calls still
pending when stdin closes get 30 seconds (`SHUTDOWN_GRACE_MS`) to finish
what they were writing; then they are logged as abandoned and the server
exits anyway. A closed stdout (`EPIPE`) exits 0; any other stdout failure
exits 1, since nobody can be answered. A failure outside a `tools/call` (an
unawaited rejection, a timer callback that throws) is logged as `server kept
alive` and the server goes on serving: losing the process would lose
`write_note` for the rest of the session. Diagnostics go to stderr with a
`staged-mcp:` prefix. A session id that is empty, contains `/`, or is `.` or
`..` is refused with exit code 2.

## The per-session scratch area

`~/.staged/sessions/<session-id>/` is one directory per Staged session, a
scratch area shared by every tool of that session:

- A tool creates it the first time it writes (`ensureSessionDir`, mode 700).
  The server never creates it on its own. For a note session the laptop
  creates it at run start, in the command that settles the outbox, so the
  agent can write `draft.md` into it; the end-of-run clear removes it again,
  so a session whose calls were all rejected leaves nothing behind.
- Each tool owns its own file names in it. Keep them distinct per tool.
- A file can be the agent's to write rather than the tool's. `draft.md` is:
  the agent writes and edits it with its own file tools, and `write_note`
  only reads it (a regular file, UTF-8, non-empty, at most 1 MiB) and
  answers a tool error naming the absolute path otherwise. Read such a file
  the way `readDraft` does: open it once with `O_NONBLOCK`, take the type
  and size from the descriptor, and read at most one byte past the cap, all
  inside one `try` so that an unreadable or vanished file is a rejected
  call and not a thrown error. The open follows symbolic links. The steps
  mirror `note_draft::read_draft` on the laptop, including the dropped
  leading byte order mark and a trim that matches Rust's `str::trim`
  (`rustTrim`), so one file gives one body on both transports.
- Write atomically (`writeAtomic`: temp file, `fsync`, rename, mode 600).
  When the laptop must read several files as one unit, bracket them with a
  marker the way `write_note` does with `done`: remove it before the first
  file changes, write it back last, and put a new random token in it on
  every call. The laptop reads the marker before and after the files and
  trusts them only when the token is the same both times. A marker without
  a token cannot show that a whole call ran between two of the laptop's
  reads.
- The laptop reads the directory over `ws exec` with `sh -c` scripts and
  parses stdout only (`sq blox ws exec` prints its own log line on stderr).
  File content travels base64-encoded so it cannot break the line framing.
  The session id is validated to `[A-Za-z0-9_-]` before it is interpolated
  into a script (`remote_staged_mcp::validate_session_id`,
  `session_dir_expr`).
- The laptop clears what it has ingested. A clear removes only that tool's
  files and then `rmdir`s the directory, which fails harmlessly while
  another tool's files are still there.

`write_note` is the first tenant, with four files:

```text
draft.md   the note body, written and edited by the agent; write_note publishes it
note.md    the published body: a copy of draft.md taken by the call
meta.json  title, suggested next steps, turn id, timestamp, script version
done       marker holding the call's token, removed first and written last
```

`note.md`, `meta.json` and `done` are the outbox the laptop collects.
`draft.md` is never collected: the laptop seeds it at run start for an
amendment and removes it whenever it clears the outbox, together with
`.draft.md.seed`, the temp file a seed is decoded into. Because the laptop
creates the directory at run start, an empty directory or an unpublished
`draft.md` can remain in the states that keep the outbox, until a Resume
or the sweep clears it. The layout,
collector, seed rule and clear policy are described in
[`note-delivery.md`](./note-delivery.md).

## Adding a tool

On the workstation, in `staged-mcp.cjs`:

1. Add a tool object in part 2 of the file, with `name`, `description`,
   `inputSchema`, and `call(context, args)` returning `{ text, isError }` or
   a promise of it. A synchronous `call` is answered at once; one that
   spawns a process or awaits I/O can return a promise and the server
   replies when it settles. `context` carries `sessionId`, `turnId` and
   `sessionDir`. The optional `instructions` sentence is appended to the
   `initialize` instructions; it may be a function of `context`, which is
   how `write_note` names the session's `draft.md` by absolute path. The
   optional `failureText(error)` is the
   result text when `call` throws or its promise rejects. Keep `failureText`
   simple: if it throws, the agent gets the default `<name> failed:
<message>` text instead. A `call` that holds a long-lived handle need not
   `unref()` it, since the server exits on stdin close once the call has
   settled, but it should settle its promise even when what it started
   hangs, or the call is abandoned after the shutdown grace.
2. Add it to `TOOLS`.
3. Use the shared plumbing for files: `ensureSessionDir`, `writeAtomic`,
   `removeIfPresent`.

Nothing has to change for the upload. The new script has a new checksum, so
every workstation gets it on its next session. Bump `SERVER_VERSION` only
when a reader on the laptop needs to tell the old shape of a tool's output
from the new one, or when a tool's contract changes (version 3: `write_note`
takes no `body` and publishes `draft.md`; version 4: the draft read drops a
leading byte order mark, trims as Rust does, reports an unreadable draft as
a rejected call, and the saved-note text counts UTF-8 bytes as the laptop
does; version 5: the saved-note text calls that count bytes, not
characters; version 6: `write_note` writes a random token into `done`, which
was an empty file before, so the collector can tell two calls' markers
apart; version 7: the saved-note text and the tool description ask the agent
to close the turn with a bare "Done" message, which the chat pane hides).

On the laptop:

1. Add a submodule of `remote_staged_mcp` that reads what the tool writes: a
   shell script builder, a parser and a typed result, on top of
   `RemoteExec`, `validate_session_id` and `session_dir_expr`.
   `note_outbox` is the model.
2. Decide when the session runner reads and clears it, and widen the attach
   condition if the tool serves more than note sessions.
3. If a local rmcp server offers the same tool, keep the name, description
   and schema in step and pin them with a test, as
   `write_note_schema_matches_the_rust_tool_schema` does.
4. Test the tool as a subprocess with `test_support::run_script`, test the
   reader against a recorded runner and against a local `sh`
   (`test_support::local_exec`), and add the tool to the expected list in
   `script_serves_the_tool_registry_over_mcp`. Required as a module instead
   of run, the script exports `TOOLS`, `callTool`, `handleLine`, `serveLine`
   and `main`, which is how
   `script_serves_asynchronous_tools_and_survives_stray_failures` adds
   throwaway tools through a driver before starting the server.
   `test_support::run_node_lines` fails a run that is still alive 60 seconds
   after stdin closed, so a tool that keeps the process alive fails its test.

## How to test manually

1. Confirm the workstation has `node`: `sq blox ws exec <ws> -- node --version`.
2. Start a remote note session. The log shows `Staged MCP on Blox attached
(turn <id>)`. On the workstation, `~/.staged/bin/staged-mcp.cjs` and
   `staged-mcp.sha256` exist, and `staged-note-mcp.cjs` /
   `staged-note-mcp.sha256` do not. A second session logs no `Installed the
Staged MCP on Blox` line and uploads nothing, because the probe's
   checksum matches.
3. Drive the server by hand on the workstation:
   `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | node
~/.staged/bin/staged-mcp.cjs test-session t1`. The `write_note` schema
   lists `title` and `suggested_next_steps` and no `body`. An `initialize`
   request answers with a `serverInfo.version` equal to `SERVER_VERSION` in
   the `staged-mcp.cjs` of the build under test (as a string) and
   instructions that name
   `/home/<user>/.staged/sessions/test-session/draft.md`.

## Automated coverage

- `remote_staged_mcp::tests`: the checksum of the embedded script (and that
  it is the file in `resources/remote`), session id validation, the install
  probe, the install sequence (upload when stale, skip when current, fail
  without `node`), the legacy cleanup (in the probe script, and against a
  local `sh` both on the first install and after an older build reinstalled
  its copy), the stdio injection, the install scripts against a local `sh`,
  protocol-version negotiation, the generic MCP methods over the tool
  registry (including the `initialize` instructions naming `draft.md` by
  absolute path), asynchronous tools (a promise settled by a later `release` call
  is answered after the requests dispatched before it, rejections with and
  without `failureText`, a `failureText` that throws, a resolve to nothing,
  stray failures outside a call, and an interval handle that does not keep
  the process alive after stdin closes), and a request-handler throw
  answered with `-32603` while the rest of the chunk is served.
- `remote_staged_mcp::note_outbox::tests`: `write_note` and its outbox (see
  [`note-delivery.md`](./note-delivery.md)).
- `acp-client` driver tests: stdio servers pass through
  `AcpDriver::for_workspace` for the `goose` and `claude` providers.
