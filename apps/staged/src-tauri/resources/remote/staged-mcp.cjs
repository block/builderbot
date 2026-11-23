#!/usr/bin/env node
"use strict";
/**
 * Staged MCP on Blox: Staged's MCP server on a Blox workstation.
 *
 * A remote agent cannot reach the MCP servers Staged runs on the laptop's
 * loopback, and the laptop may disconnect while the agent is still working.
 * So Staged uploads this file to the workstation as
 * `~/.staged/bin/staged-mcp.cjs` and injects it into the agent's
 * `session/new` as a stdio MCP server named `staged`:
 *
 *     node staged-mcp.cjs <staged-session-id> <turn-id>
 *
 * It speaks newline-delimited JSON-RPC 2.0 on stdin/stdout (the MCP stdio
 * transport) and implements just enough of MCP to serve a few tools:
 * `initialize`, `ping`, `tools/list`, `tools/call`, empty `resources/list`
 * and `prompts/list`, and it ignores notifications.
 *
 * Tools keep what they produce in the session's scratch directory,
 * `~/.staged/sessions/<staged-session-id>/`, which the laptop reads over
 * `ws exec` after the turn, or on its next sweep if it was disconnected. The
 * whole point of this server living on the workstation is that what a tool
 * writes there survives that disconnect.
 *
 * The file has three parts:
 *
 *   1. Shared plumbing: logging, JSON-RPC framing, the session directory and
 *      atomic file writes.
 *   2. Tools, one object each, collected in `TOOLS`. `write_note` is the
 *      first.
 *   3. The MCP server, which answers requests from `TOOLS`.
 *
 * Adding a tool: write an object with `name`, `description`, `inputSchema`,
 * an optional `instructions` sentence (appended to the `initialize`
 * instructions; a string, or a function of `context` for text that names a
 * path), an optional `failureText(error)` (the result text when
 * `call` throws or rejects), and `call(context, args)` returning
 * `{ text, isError }` or a promise of it. `context` carries `sessionId`,
 * `turnId` and `sessionDir`. Then add the object to `TOOLS`. See
 * `apps/staged/docs/staged-mcp-on-blox.md` for the laptop side.
 *
 * Required as a module (`require("./staged-mcp.cjs")`) the file exports its
 * registry and server functions instead of starting, so a test driver can add
 * fixture tools before calling `main`.
 *
 * No third-party dependencies: Node's standard library only, CommonJS (the
 * `.cjs` extension keeps it CommonJS whatever `package.json` is nearby), and
 * nothing newer than Node 18 syntax so an older workstation image still runs it.
 */

const crypto = require("crypto");
const fs = require("fs");
const os = require("os");
const path = require("path");
const readline = require("readline");

// ---------------------------------------------------------------------------
// 1. Shared plumbing
// ---------------------------------------------------------------------------

// Reported as `serverInfo.version` and stamped by tools into what they write
// (`write_note`'s `meta.json` keeps it under its original `helperVersion` key).
//
// Version 8: `write_note` accepts a call without `title` and takes the title
// from the draft's first line, and `meta.json` holds the title exactly as the
// laptop stores it. The laptop reads the version to tell that from the title
// of an earlier script, which it still normalizes itself
// (`remote_staged_mcp::note_outbox::outbox_state`).
//
// Version 9: `title` is no longer an argument of `write_note`. The title is
// always the draft's first line, and the tool's schema, description and
// instructions say so. What a call writes into the outbox is unchanged.
//
// Version 10: the draft's title line ends at the first line ending of any
// kind, a "\r" on its own included, where it ended only at a "\n" before. A
// draft with CR-only line endings is split into title and body (it was
// rejected as holding only the title line), and a title never holds a "\r".
// The outbox format is unchanged.
const SERVER_VERSION = 10;
const SERVER_NAME = "staged";

// MCP protocol revisions this server implements (newest first). `initialize`
// answers with the client's version when it is one of these, otherwise with
// the newest one we speak — never with a version we have not implemented.
const SUPPORTED_PROTOCOL_VERSIONS = ["2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST_PROTOCOL_VERSION = SUPPORTED_PROTOCOL_VERSIONS[0];

/** The protocol version to answer an `initialize` carrying `requested`. */
function negotiateProtocolVersion(requested) {
  if (typeof requested === "string" && SUPPORTED_PROTOCOL_VERSIONS.indexOf(requested) !== -1) {
    return requested;
  }
  return LATEST_PROTOCOL_VERSION;
}

function log(message) {
  process.stderr.write("staged-mcp: " + message + "\n");
}

function writeFrame(payload) {
  process.stdout.write(JSON.stringify(payload) + "\n");
}

function reply(id, result) {
  writeFrame({ jsonrpc: "2.0", id: id, result: result });
}

function replyError(id, code, message) {
  writeFrame({ jsonrpc: "2.0", id: id, error: { code: code, message: message } });
}

/** The session's scratch directory; tools create it only when they write. */
function sessionDir(sessionId) {
  return path.join(os.homedir(), ".staged", "sessions", sessionId);
}

/** Create the session's scratch directory (owner-only) and return it. */
function ensureSessionDir(context) {
  fs.mkdirSync(context.sessionDir, { recursive: true, mode: 0o700 });
  return context.sessionDir;
}

/** Write `data` to `directory/name` via a temp file and rename. */
function writeAtomic(directory, name, data) {
  const tmpPath = path.join(
    directory,
    "." + name + "." + process.pid + "." + Date.now() + "." + Math.random().toString(16).slice(2)
  );
  const fd = fs.openSync(tmpPath, "w", 0o600);
  try {
    fs.writeSync(fd, data);
    fs.fsyncSync(fd);
  } finally {
    fs.closeSync(fd);
  }
  try {
    fs.renameSync(tmpPath, path.join(directory, name));
  } catch (error) {
    try {
      fs.unlinkSync(tmpPath);
    } catch (_ignored) {
      // nothing to clean up
    }
    throw error;
  }
}

/** Unlink `filePath`, treating a missing file as already removed. */
function removeIfPresent(filePath) {
  try {
    fs.unlinkSync(filePath);
  } catch (error) {
    if (!error || error.code !== "ENOENT") throw error;
  }
}

// ---------------------------------------------------------------------------
// 2. Tools
// ---------------------------------------------------------------------------

// write_note: publish the draft into the note outbox.
//
// The note is the session's draft file, `draft.md` in the session directory,
// which the agent writes with its own file tools (Staged seeds it with the
// stored note for an amendment and leaves it absent for a fresh note). A
// call reads the draft and replaces the session's note, atomically (temp
// file + rename), in the session directory.
//
// The draft's first line is the note's title, a `# Title` heading, and the
// rest is the body (`splitDraftTitle`). The title is not an argument. The
// two are published apart: `meta.json` holds the title and `note.md` the
// body without the title line:
//
//     draft.md   the agent's draft (the laptop seeds and clears it; this
//                server only reads it)
//     note.md    the published note body (markdown, without the title)
//     meta.json  {"title", "suggestedNextSteps", "turn", "writtenAt", "helperVersion"}
//     done       marker: the turn delivered a note. It holds the call's
//                generation token, 32 hex characters, new on every call
//
// A `title` argument is still read, though the schema does not list it, for
// a session resumed with the earlier instructions in its context: one that
// agrees with the draft's title line is accepted and one that does not is
// rejected (`staleTitleAgrees`).
//
// `done` is removed before a rewrite starts and recreated after both files
// are in place, so the marker is absent for the whole of a rewrite. That
// alone does not protect a collector, which reads the files in separate
// steps: a whole second call can fit between its read of `meta.json` and
// its read of `note.md`, and the marker is present before and after. So the
// collector reads the marker before and after the files and takes them as
// one note only when it finds the same token both times. The laptop's
// reader is `remote_staged_mcp::note_outbox`.

// Must stay word-for-word in step with `WRITE_NOTE_DESCRIPTION` in
// `apps/staged/src-tauri/src/note_tool.rs`.
const WRITE_NOTE_DESCRIPTION =
  "Publish the note this session is producing. This call is the deliverable: " +
  "Staged stores the current contents of your note draft file (the path is in your instructions) " +
  "as the note, and does not read your assistant text. The file's first line must be the note's " +
  "title as a level-1 markdown heading (`# Title`), and everything below that line is the note " +
  "body. Write the whole note into the draft file first. Calling again republishes the file, " +
  "and the last call of the turn wins. After the result says the note is saved, send a simple " +
  '"Done" message with no other text to end your turn, rather than summarising the work done. ' +
  "If the user asked you a question, answer it before making this call.";

// Mirrors the schema rmcp derives from `WriteNoteParams`; the Rust test
// `write_note_schema_matches_the_rust_tool_schema` checks property names and
// required fields. There is no `body` and no `title`: both are the draft
// file. Nothing is required.
const WRITE_NOTE_INPUT_SCHEMA = {
  type: "object",
  properties: {
    suggested_next_steps: {
      description:
        "What the user might want to do next. Keep both fields to a few words and use " +
        "null when there is no clear next action.",
      anyOf: [
        {
          type: "object",
          properties: {
            suggestedNextCommitStep: { type: ["string", "null"] },
            suggestedNextNoteStep: { type: ["string", "null"] },
          },
        },
        { type: "null" },
      ],
    },
  },
};

// The draft file name and the largest draft a call publishes; both match
// `note_draft` on the laptop.
const DRAFT_FILE_NAME = "draft.md";
const DRAFT_MAX_BYTES = 1024 * 1024;

/** The session's draft path, as the prompt spells it out to the agent. */
function draftPath(context) {
  return path.join(context.sessionDir, DRAFT_FILE_NAME);
}

/** `write_note rejected: <what>, then call write_note again.` naming the draft. */
function draftRejection(what) {
  return "write_note rejected: " + what + ", then call write_note again.";
}

/**
 * Whether `code` is a character Rust's `str::trim` strips (the Unicode
 * White_Space property), which is how the laptop trims a draft. JavaScript's
 * `trim()` is not the same set: it also strips U+FEFF and leaves U+0085.
 */
function isRustWhitespace(code) {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

/** `text` trimmed exactly as Rust's `str::trim` would trim it. */
function rustTrim(text) {
  let start = 0;
  let end = text.length;
  while (start < end && isRustWhitespace(text.charCodeAt(start))) start++;
  while (end > start && isRustWhitespace(text.charCodeAt(end - 1))) end--;
  return text.slice(start, end);
}

/** Up to `limit` bytes from the open file `fd`, read from its start. */
function readUpTo(fd, limit) {
  const chunks = [];
  let total = 0;
  const chunk = Buffer.allocUnsafe(64 * 1024);
  while (total < limit) {
    const read = fs.readSync(fd, chunk, 0, Math.min(chunk.length, limit - total), null);
    if (read === 0) break;
    chunks.push(Buffer.from(chunk.subarray(0, read)));
    total += read;
  }
  return Buffer.concat(chunks, total);
}

/**
 * Read and validate the draft: a regular file, at most DRAFT_MAX_BYTES,
 * UTF-8, non-empty once a leading byte order mark and the surrounding
 * whitespace are removed. Returns `{ draft }`, the whole draft with its
 * title line, or `{ error }` with the same wording as
 * `note_draft::DraftError::describe` on the laptop.
 *
 * Same steps, in the same order, as `note_draft::read_draft`. The file is
 * opened once and everything is asked of the descriptor, so the file that is
 * checked is the file that is read and the read stops one byte past the cap.
 * `O_NONBLOCK` keeps the open from waiting on a FIFO that has no writer. The
 * open follows symbolic links, so a draft that is a link to a regular file
 * publishes that file. Every failure of the open, the stat or the read is
 * reported as a rejected call naming the path, never thrown: a draft the
 * server may not read, or one that vanished, gets the laptop's wording.
 */
function readDraft(context) {
  const filePath = draftPath(context);
  const oversized = function (size) {
    return {
      error: draftRejection(
        "the draft file `" +
          filePath +
          "` is " +
          size +
          " bytes, over the " +
          DRAFT_MAX_BYTES +
          " byte limit. Shorten it"
      ),
    };
  };
  let bytes;
  let fd = null;
  try {
    fd = fs.openSync(filePath, fs.constants.O_RDONLY | (fs.constants.O_NONBLOCK || 0));
    const stat = fs.fstatSync(fd);
    if (!stat.isFile()) {
      return {
        error: draftRejection(
          "`" + filePath + "` is not a regular file. The draft must be a plain file holding the note"
        ),
      };
    }
    if (stat.size > DRAFT_MAX_BYTES) return oversized(stat.size);
    bytes = readUpTo(fd, DRAFT_MAX_BYTES + 1);
    if (bytes.length > DRAFT_MAX_BYTES) {
      // The file grew past the cap after the size check.
      return oversized(Math.max(fs.fstatSync(fd).size, bytes.length));
    }
  } catch (error) {
    if (error && error.code === "ENOENT") {
      return {
        error: draftRejection(
          "the draft file `" +
            filePath +
            "` does not exist. This file is your note: write the full note into it as markdown, " +
            "starting with the title as a `# ` heading on the first line"
        ),
      };
    }
    return {
      error: draftRejection(
        "the draft file `" +
          filePath +
          "` could not be read: " +
          (error && error.message ? error.message : error)
      ),
    };
  } finally {
    if (fd !== null) {
      try {
        fs.closeSync(fd);
      } catch (_error) {
        // Nothing to do for a descriptor that will not close.
      }
    }
  }
  let text;
  try {
    // `ignoreBOM` keeps a leading U+FEFF in the output, so the one mark that
    // is dropped is dropped here, in step with the laptop.
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch (_error) {
    return {
      error: draftRejection("the draft file `" + filePath + "` is not valid UTF-8. Rewrite it as UTF-8 text"),
    };
  }
  // One leading U+FEFF is an encoding mark some editors write, not note text.
  if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
  const draft = rustTrim(text);
  if (draft.length === 0) {
    return {
      error: draftRejection(
        "the draft file `" +
          filePath +
          "` is empty. Write the full note into it as markdown, starting with the title as a " +
          "`# ` heading on the first line"
      ),
    };
  }
  return { draft: draft };
}

/**
 * Split a draft that opens with its title into `{ title, body }`, or return
 * `{ error }` with the wording of `DraftError::MissingTitle` and
 * `DraftError::TitleOnly` on the laptop. `draft` is what `readDraft`
 * returns, so its first line is the file's first non-blank line.
 *
 * The same rule as `note_draft::split_title`, which the laptop's handler
 * applies to every call:
 *
 *   1. The first line is the text up to the first line ending, which is a
 *      "\n", a "\r\n" or a "\r" on its own, as in CommonMark.
 *   2. It must be one `#` followed by a space or a tab. The title is the rest
 *      of the line, trimmed as Rust trims, and must not be empty.
 *   3. The body is everything after that line ending, trimmed, and must not
 *      be empty.
 *
 * Only that line is consumed: `##` is a section and never a title, the title
 * text is kept as written (inline markdown, a `#` of its own and the
 * whitespace inside it included), and a later `# Heading` is body text. A
 * title never holds a "\r" or a "\n". U+2028 and the like do not end a line
 * in markdown and are title text.
 * `write_note_reads_the_same_drafts_on_both_transports` runs both
 * implementations over the same drafts.
 */
function splitDraftTitle(context, draft) {
  const filePath = draftPath(context);
  const lineEnd = draft.search(/[\r\n]/);
  const firstLine = lineEnd === -1 ? draft : draft.slice(0, lineEnd);
  // A "\r\n" is one line ending, not two.
  const lineEndLength = draft.startsWith("\r\n", lineEnd) ? 2 : 1;
  const rest = lineEnd === -1 ? "" : draft.slice(lineEnd + lineEndLength);
  const afterMarker = firstLine.charCodeAt(1);
  const title =
    firstLine.charCodeAt(0) === 0x23 && (afterMarker === 0x20 || afterMarker === 0x09)
      ? rustTrim(firstLine.slice(1))
      : "";
  if (title.length === 0) {
    return {
      error: draftRejection(
        "the draft file `" +
          filePath +
          "` does not start with the note's title. Its first non-blank line must be a " +
          "level-1 markdown heading: `# ` followed by the title on one line (not `##`, not a " +
          "line underlined with `===`, and nothing above it). Add that line at the top and " +
          "leave the rest of the note below it"
      ),
    };
  }
  const body = rustTrim(rest);
  if (body.length === 0) {
    return {
      error: draftRejection(
        "the draft file `" +
          filePath +
          "` holds only the title line. Write the note body in markdown below the `# ` title line"
      ),
    };
  }
  return { title: title, body: body };
}

/**
 * Whether a `title` argument passed under the earlier contract names the
 * title the draft's first line holds (`headingTitle`, as `splitDraftTitle`
 * returns it).
 *
 * The same rule as `stale_title_agrees` in `note_tool.rs` on the laptop: it
 * agrees when it is a string that, trimmed as Rust trims, is the heading
 * title as written, or is that title behind one `#` (the earlier contract
 * stripped one leading `#` from the argument). Nothing more is stripped, so
 * `## Plan` agrees with a draft that opens `# # Plan` and not with one that
 * opens `# Plan`. Anything else, an empty string and a value that is not a
 * string included, does not agree.
 * `write_note_reads_the_same_drafts_on_both_transports` runs both
 * implementations over the same calls.
 */
function staleTitleAgrees(argument, headingTitle) {
  if (typeof argument !== "string") return false;
  const trimmed = rustTrim(argument);
  if (trimmed === headingTitle) return true;
  return trimmed.charCodeAt(0) === 0x23 && rustTrim(trimmed.slice(1)) === headingTitle;
}

function cleanStep(value) {
  if (value === null || value === undefined) return null;
  if (typeof value !== "string") {
    throw new Error("suggested next steps must be strings or null");
  }
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

/** Return the camelCase next-steps object, or null when omitted. */
function normalizeSteps(raw) {
  if (raw === null || raw === undefined) return null;
  if (typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("`suggested_next_steps` must be an object or null");
  }
  const commit =
    raw.suggestedNextCommitStep !== undefined
      ? raw.suggestedNextCommitStep
      : raw.suggested_next_commit_step;
  const note =
    raw.suggestedNextNoteStep !== undefined ? raw.suggestedNextNoteStep : raw.suggested_next_note_step;
  return {
    suggestedNextCommitStep: cleanStep(commit),
    suggestedNextNoteStep: cleanStep(note),
  };
}

function savedMessage(title, bodyLength) {
  // Must stay word-for-word in step with `note_tool::saved_message` on the
  // laptop. The chat pane hides the quoted closing word after a saved call
  // (`noteDoneMessage.ts`), so that word has to stay the one its filter
  // accepts.
  return (
    'Saved note "' +
    title +
    '" (' +
    bodyLength +
    " bytes from the draft file). If the user asked you a question this turn and you have not " +
    "answered it in chat yet, answer it now, briefly. Otherwise send a simple " +
    '"Done" message with no other text to end your turn, rather than summarising the work done.'
  );
}

/**
 * A token for one `write_note` call's `done` marker: 32 lowercase hex
 * characters, random, so no two calls write the same one. The collector
 * prints it as part of a line of its own output, so it stays within letters
 * and digits and has no newline.
 */
function generationToken() {
  return crypto.randomBytes(16).toString("hex");
}

/**
 * Validate the arguments, read the draft and write the note outbox. Returns
 * {text, isError}.
 *
 * The title is the draft's first line and the body is the rest
 * (`splitDraftTitle`), as on the laptop's handler. A call that also passes
 * the former `title` argument (anything but null) is accepted when the
 * argument agrees with that line and rejected when it does not
 * (`staleTitleAgrees`): ignoring it would silently drop a rename asked for
 * by an agent whose context still holds the earlier instructions.
 */
function writeNote(context, args) {
  if (args === null || typeof args !== "object" || Array.isArray(args)) {
    return { text: "write_note rejected: arguments must be an object.", isError: true };
  }
  let steps;
  try {
    steps = normalizeSteps(args.suggested_next_steps);
  } catch (error) {
    return { text: "write_note rejected: " + error.message + ".", isError: true };
  }
  // The draft is read before anything is written, so a bad draft leaves the
  // outbox exactly as it was.
  const read = readDraft(context);
  if (read.error) {
    log("write_note could not read the draft for session " + context.sessionId);
    return { text: read.error, isError: true };
  }
  // A draft with no title line, or nothing under it, is rejected like an
  // unreadable one. This comes before the stale `title` check, as on the
  // laptop, so such a call is told to add the line.
  const split = splitDraftTitle(context, read.draft);
  if (split.error) {
    log("write_note found no title and body in the draft for session " + context.sessionId);
    return { text: split.error, isError: true };
  }
  const title = split.title;
  const body = split.body;
  if (args.title !== undefined && args.title !== null && !staleTitleAgrees(args.title, title)) {
    log("write_note was passed a title that differs from the draft's for session " + context.sessionId);
    // Must stay word-for-word in step with `SubmissionError::StaleTitle` in
    // `note_tool.rs` on the laptop.
    return {
      text:
        "write_note rejected: `title` is no longer an argument. The note's title is the first " +
        "line of the draft file `" +
        draftPath(context) +
        '`, currently "' +
        title +
        '", which differs from the `title` you passed. Edit that line if the title should ' +
        "change, then call write_note again without `title`.",
      isError: true,
    };
  }

  const directory = ensureSessionDir(context);
  // `meta.json` holds the title as the laptop stores it: as written.
  const meta = {
    title: title,
    suggestedNextSteps: steps,
    turn: context.turnId,
    writtenAt: Math.floor(Date.now() / 1000),
    helperVersion: SERVER_VERSION,
  };
  // Drop the marker of any earlier call first, then body, then meta, then
  // the marker again, holding a token no other call has written: a collector
  // that reads the same token before and after the other two files can rely
  // on them being complete *and* from the same call. A missing marker is a
  // rewrite between its renames; a different token is a whole call that ran
  // between the collector's reads.
  removeIfPresent(path.join(directory, "done"));
  writeAtomic(directory, "note.md", Buffer.from(body, "utf8"));
  writeAtomic(directory, "meta.json", Buffer.from(JSON.stringify(meta), "utf8"));
  writeAtomic(directory, "done", Buffer.from(generationToken(), "ascii"));
  log(
    "saved note " + JSON.stringify(title) + " (" + body.length + " chars) for session " + context.sessionId
  );
  // The laptop reports the body's UTF-8 length (`note_tool::saved_message`).
  return { text: savedMessage(title, Buffer.byteLength(body, "utf8")), isError: false };
}

const WRITE_NOTE_TOOL = {
  name: "write_note",
  description: WRITE_NOTE_DESCRIPTION,
  inputSchema: WRITE_NOTE_INPUT_SCHEMA,
  instructions: function (context) {
    return (
      "Your note draft file is `" +
      draftPath(context) +
      "`. This file is your note: write it, edit it, and when it is ready call write_note " +
      "(suggested next steps) to publish it; your assistant text is not saved. Its first line " +
      "must be the note's title as a level-1 markdown heading (`# Title`)."
    );
  },
  failureText: function (error) {
    return "write_note failed to save the note: " + error.message;
  },
  call: writeNote,
};

/** Every tool this server offers, in `tools/list` order. */
const TOOLS = [WRITE_NOTE_TOOL];

// ---------------------------------------------------------------------------
// 3. The MCP server
// ---------------------------------------------------------------------------

function findTool(name) {
  for (let i = 0; i < TOOLS.length; i++) {
    if (TOOLS[i].name === name) return TOOLS[i];
  }
  return null;
}

/** The `initialize` result for a client asking for `requestedVersion`. */
function initializeResult(context, requestedVersion) {
  const result = {
    protocolVersion: negotiateProtocolVersion(requestedVersion),
    capabilities: { tools: {} },
    serverInfo: { name: SERVER_NAME, version: String(SERVER_VERSION) },
  };
  const instructions = TOOLS.map(function (tool) {
    return typeof tool.instructions === "function" ? tool.instructions(context) : tool.instructions;
  }).filter(Boolean);
  if (instructions.length > 0) result.instructions = instructions.join(" ");
  return result;
}

function isThenable(value) {
  return value !== null && typeof value === "object" && typeof value.then === "function";
}

function describeError(error) {
  return error && error.stack ? error.stack : String(error);
}

/** A `tools/call` result carrying `outcome.text` and `outcome.isError`. */
function toolResult(outcome) {
  return { content: [{ type: "text", text: outcome.text }], isError: outcome.isError };
}

/** `<name> failed: <message>`, the failure text a tool gets by default. */
function defaultFailureText(tool, error) {
  return tool.name + " failed: " + (error && error.message ? error.message : error);
}

/**
 * The `tools/call` result for `tool` failing with `error`: its `failureText`,
 * or `<name> failed: <message>`, as an `isError` result, logged.
 *
 * A `failureText` that itself throws falls back to the default text, so a
 * failing tool always gets a reply and never escapes into the request
 * handler.
 */
function toolFailure(tool, error) {
  // Report every failure to the agent rather than dying mid-turn.
  log(tool.name + " failed: " + describeError(error));
  let text = defaultFailureText(tool, error);
  if (tool.failureText) {
    try {
      text = tool.failureText(error);
    } catch (textError) {
      log(tool.name + " failed while describing its failure: " + describeError(textError));
    }
  }
  return toolResult({ text: text, isError: true });
}

/** The result for what `tool.call` returned or resolved to. */
function settleOutcome(tool, outcome) {
  if (outcome === null || typeof outcome !== "object" || typeof outcome.text !== "string") {
    return toolFailure(tool, new Error("returned no { text, isError } result"));
  }
  return toolResult(outcome);
}

/**
 * Run `tool` and turn its outcome (or failure) into a `tools/call` result.
 *
 * `tool.call` may return the outcome or a promise of it. A synchronous tool
 * (`write_note`) gets its result back synchronously; an asynchronous one
 * gets a promise that resolves to the result once the tool settles. A throw
 * and a rejection take the same `failureText` path, so the promise never
 * rejects.
 */
function callTool(tool, context, args) {
  let outcome;
  try {
    outcome = tool.call(context, args);
  } catch (error) {
    return toolFailure(tool, error);
  }
  if (isThenable(outcome)) {
    return outcome.then(
      function (settled) {
        return settleOutcome(tool, settled);
      },
      function (error) {
        return toolFailure(tool, error);
      }
    );
  }
  return settleOutcome(tool, outcome);
}

/**
 * How long the server keeps running after stdin closes while `tools/call`
 * promises are still pending, so an asynchronous tool can finish what it was
 * writing. The agent has already gone, so nobody reads a reply after this.
 */
const SHUTDOWN_GRACE_MS = 30000;

/** Exit once every reply written so far has left stdout. */
function exitAfterFlush(code) {
  // Pipes are asynchronous on some platforms; the empty write's callback
  // runs after everything queued before it has been written.
  process.stdout.write("", function () {
    process.exit(code);
  });
}

/**
 * The `tools/call` promises still pending, and whether stdin has closed.
 * The server exits when both stdin is closed and nothing is pending, so a
 * tool holding a live handle (a child process, an interval) cannot keep the
 * process alive after the agent has gone.
 */
const shutdown = { pending: 0, closing: false, graceTimer: null };

function trackPendingCall(promise) {
  shutdown.pending += 1;
  const settled = function () {
    shutdown.pending -= 1;
    if (shutdown.closing && shutdown.pending === 0) {
      if (shutdown.graceTimer !== null) clearTimeout(shutdown.graceTimer);
      exitAfterFlush(0);
    }
  };
  promise.then(settled, settled);
}

/** stdin has closed: exit now, or once the pending calls have settled. */
function beginShutdown() {
  shutdown.closing = true;
  if (shutdown.pending === 0) {
    exitAfterFlush(0);
    return;
  }
  log("stdin closed with " + shutdown.pending + " call(s) pending; waiting for them");
  shutdown.graceTimer = setTimeout(function () {
    log("abandoning " + shutdown.pending + " call(s) still pending after " + SHUTDOWN_GRACE_MS + " ms");
    exitAfterFlush(0);
  }, SHUTDOWN_GRACE_MS);
}

/**
 * The `id` of the request on `rawLine`, for an error reply after
 * `handleLine` threw, or `undefined` when the line carries none.
 */
function requestIdOf(rawLine) {
  try {
    const message = JSON.parse(rawLine);
    if (message !== null && typeof message === "object" && !Array.isArray(message)) {
      return Object.prototype.hasOwnProperty.call(message, "id") ? message.id : undefined;
    }
  } catch (_error) {
    // Not JSON: handleLine already answered -32700 before it could throw.
  }
  return undefined;
}

/**
 * Serve one stdin line. A throw out of `handleLine` (a tool whose property
 * getter throws, say) answers `-32603` for the request's id, or is only
 * logged when the line has none, so the request is never left unanswered.
 */
function serveLine(context, rawLine) {
  try {
    handleLine(context, rawLine);
  } catch (error) {
    log("request failed: " + describeError(error));
    const id = requestIdOf(rawLine);
    if (id === undefined) return;
    replyError(id, -32603, "Internal error: " + (error && error.message ? error.message : error));
  }
}

function handleLine(context, rawLine) {
  const line = rawLine.trim();
  if (line.length === 0) return;
  let message;
  try {
    message = JSON.parse(line);
  } catch (_error) {
    replyError(null, -32700, "Parse error");
    return;
  }
  if (message === null || typeof message !== "object" || Array.isArray(message)) {
    replyError(null, -32600, "Invalid Request");
    return;
  }
  const method = message.method;
  const id = message.id;
  const params = message.params || {};
  const isNotification = !Object.prototype.hasOwnProperty.call(message, "id");

  // A response to something we never sent, or a notification
  // (notifications/initialized, notifications/cancelled, ...): nothing to do.
  if (method === undefined || method === null) return;
  if (isNotification) return;

  if (method === "initialize") {
    reply(id, initializeResult(context, params.protocolVersion));
  } else if (method === "ping") {
    reply(id, {});
  } else if (method === "tools/list") {
    reply(id, {
      tools: TOOLS.map(function (tool) {
        return { name: tool.name, description: tool.description, inputSchema: tool.inputSchema };
      }),
    });
  } else if (method === "tools/call") {
    const tool = findTool(params.name);
    if (tool === null) {
      replyError(id, -32602, "Unknown tool: " + params.name);
      return;
    }
    const result = callTool(tool, context, params.arguments || {});
    if (isThenable(result)) {
      // An asynchronous tool answers when it settles; requests arriving in
      // the meantime are served first, so replies may leave out of order
      // (JSON-RPC matches them by id). `callTool`'s promise never rejects.
      trackPendingCall(
        result.then(function (settled) {
          reply(id, settled);
        })
      );
      return;
    }
    reply(id, result);
  } else if (method === "resources/list" || method === "prompts/list") {
    const result = {};
    result[method.split("/")[0]] = [];
    reply(id, result);
  } else {
    replyError(id, -32601, "Method not found: " + method);
  }
}

function main(argv) {
  if (argv.length < 1) {
    log("usage: staged-mcp.cjs <staged-session-id> <turn-id>");
    process.exitCode = 2;
    return;
  }
  const sessionId = argv[0];
  const turnId = argv.length > 1 ? argv[1] : "";
  if (!sessionId || sessionId.indexOf("/") !== -1 || sessionId === "." || sessionId === "..") {
    log("refusing session id " + JSON.stringify(sessionId));
    process.exitCode = 2;
    return;
  }
  const context = { sessionId: sessionId, turnId: turnId, sessionDir: sessionDir(sessionId) };
  // A tool's stray failure (a timer callback that throws, a promise nobody
  // awaited) must not take the server down for the rest of the session: log
  // it and keep serving. The agent still gets its reply, or a failure result,
  // through `callTool`.
  process.on("uncaughtException", function (error) {
    log("uncaught exception, server kept alive: " + describeError(error));
  });
  process.on("unhandledRejection", function (reason) {
    log("unhandled rejection, server kept alive: " + describeError(reason));
  });
  process.stdout.on("error", function (error) {
    if (error && error.code === "EPIPE") process.exit(0);
    // Without stdout there is nobody to answer; leave rather than linger.
    log("stdout failed: " + describeError(error));
    process.exit(1);
  });
  const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  lines.on("line", function (line) {
    serveLine(context, line);
  });
  // stdin closing ends the server once the pending calls have settled.
  lines.on("close", beginShutdown);
}

if (require.main === module) {
  main(process.argv.slice(2));
} else {
  module.exports = {
    TOOLS: TOOLS,
    callTool: callTool,
    handleLine: handleLine,
    serveLine: serveLine,
    main: main,
  };
}
