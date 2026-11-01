#!/usr/bin/env node
"use strict";
/**
 * Staged's workstation-side `write_note` MCP server.
 *
 * Staged uploads this file to a Blox workstation at the start of a remote note
 * session and injects it into the agent's `session/new` as a stdio MCP server:
 *
 *     node staged-note-mcp.cjs <staged-session-id> <turn-id>
 *
 * It speaks newline-delimited JSON-RPC 2.0 on stdin/stdout (the MCP stdio
 * transport) and implements just enough of MCP for one tool: `initialize`,
 * `notifications/initialized`, `ping`, `tools/list`, and `tools/call`.
 *
 * Every `write_note` call replaces the outbox for the session, atomically
 * (temp file + rename), under `~/.staged/sessions/<staged-session-id>/`:
 *
 *     note.md    the note body (markdown, without the title)
 *     meta.json  {"title", "suggestedNextSteps", "turn", "writtenAt", "helperVersion"}
 *     done       marker: the turn delivered a note
 *
 * `done` is removed before a rewrite starts and recreated after both files
 * are in place, so a collector that sees it — even one that lands between
 * the renames of a second call — always reads a title and body from the
 * same call.
 *
 * The laptop collects the outbox after the turn, or on its next sweep if it
 * was disconnected while the agent was still working. The whole point of this
 * file living on the workstation is that the note survives that disconnect.
 *
 * No third-party dependencies: Node's standard library only, CommonJS (the
 * `.cjs` extension keeps it CommonJS whatever `package.json` is nearby), and
 * nothing newer than Node 18 syntax so an older workstation image still runs it.
 */

const fs = require("fs");
const os = require("os");
const path = require("path");
const readline = require("readline");

const HELPER_VERSION = 2;
const SERVER_NAME = "staged";
const TOOL_NAME = "write_note";

// MCP protocol revisions this helper implements (newest first). `initialize`
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

// Must stay word-for-word in step with `WRITE_NOTE_DESCRIPTION` in
// `apps/staged/src-tauri/src/note_tool.rs`.
const TOOL_DESCRIPTION =
  "Save the note this session is producing. This call is the deliverable: " +
  "Staged stores exactly what you pass here and does not read your assistant text. " +
  "Pass the full note each time (title, markdown body, and suggested next steps); " +
  "calling again replaces the previous note, and the last call of the turn wins. " +
  "After the result says the note is saved, end your turn without repeating the note.";

// Mirrors the schema rmcp derives from `WriteNoteParams`; the Rust test
// `helper_schema_matches_tool_schema` checks property names and required fields.
const INPUT_SCHEMA = {
  type: "object",
  properties: {
    title: {
      type: "string",
      description: "The note title, shown as its heading. Plain text, no markdown, no leading `#`.",
    },
    body: {
      type: "string",
      description:
        "The full note body in markdown, without the title. Write it as a standalone " +
        "artifact: the user's prompt is not shown with it.",
    },
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
  required: ["title", "body"],
};

function log(message) {
  process.stderr.write("staged-note-mcp: " + message + "\n");
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

function outboxDir(sessionId) {
  return path.join(os.homedir(), ".staged", "sessions", sessionId);
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
  // Same wording as `note_tool::saved_message` on the laptop.
  return (
    'Saved note "' +
    title +
    '" (' +
    bodyLength +
    " characters). The note is stored; end your turn now without repeating it in your reply."
  );
}

/** Validate the arguments and write the outbox. Returns {text, isError}. */
function handleWriteNote(sessionId, turnId, args) {
  if (args === null || typeof args !== "object" || Array.isArray(args)) {
    return { text: "write_note rejected: arguments must be an object.", isError: true };
  }
  let title = args.title;
  let body = args.body;
  if (typeof title !== "string" || typeof body !== "string") {
    return {
      text:
        "write_note rejected: `title` and `body` must be strings. " +
        "Call it again with a non-empty title and body.",
      isError: true,
    };
  }
  title = title.trim();
  if (title.startsWith("#")) title = title.slice(1).trim();
  if (title.length === 0) {
    return {
      text: "write_note rejected: `title` must not be empty. Call it again with a non-empty title and body.",
      isError: true,
    };
  }
  if (title.indexOf("\n") !== -1) {
    return {
      text: "write_note rejected: `title` must be a single line. Call it again with a one-line title.",
      isError: true,
    };
  }
  body = body.trim();
  if (body.length === 0) {
    return {
      text: "write_note rejected: `body` must not be empty. Call it again with a non-empty title and body.",
      isError: true,
    };
  }
  let steps;
  try {
    steps = normalizeSteps(args.suggested_next_steps);
  } catch (error) {
    return { text: "write_note rejected: " + error.message + ".", isError: true };
  }

  const directory = outboxDir(sessionId);
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const meta = {
    title: title,
    suggestedNextSteps: steps,
    turn: turnId,
    writtenAt: Math.floor(Date.now() / 1000),
    helperVersion: HELPER_VERSION,
  };
  // Drop the marker of any earlier call first, then body, then meta, then
  // the marker again: a collector that sees `done` can rely on the other two
  // being complete *and* from the same call, even if it reads while a
  // rewrite is between its renames.
  removeIfPresent(path.join(directory, "done"));
  writeAtomic(directory, "note.md", Buffer.from(body, "utf8"));
  writeAtomic(directory, "meta.json", Buffer.from(JSON.stringify(meta), "utf8"));
  writeAtomic(directory, "done", Buffer.alloc(0));
  log("saved note " + JSON.stringify(title) + " (" + body.length + " chars) for session " + sessionId);
  return { text: savedMessage(title, body.length), isError: false };
}

function handleLine(sessionId, turnId, rawLine) {
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
    reply(id, {
      protocolVersion: negotiateProtocolVersion(params.protocolVersion),
      capabilities: { tools: {} },
      serverInfo: { name: SERVER_NAME, version: String(HELPER_VERSION) },
      instructions:
        "Call write_note to save the note this session is producing; your assistant text is not saved.",
    });
  } else if (method === "ping") {
    reply(id, {});
  } else if (method === "tools/list") {
    reply(id, {
      tools: [{ name: TOOL_NAME, description: TOOL_DESCRIPTION, inputSchema: INPUT_SCHEMA }],
    });
  } else if (method === "tools/call") {
    if (params.name !== TOOL_NAME) {
      replyError(id, -32602, "Unknown tool: " + params.name);
      return;
    }
    let outcome;
    try {
      outcome = handleWriteNote(sessionId, turnId, params.arguments || {});
    } catch (error) {
      // Report every failure to the agent rather than dying mid-turn.
      log("write_note failed: " + (error && error.stack ? error.stack : error));
      outcome = { text: "write_note failed to save the note: " + error.message, isError: true };
    }
    reply(id, { content: [{ type: "text", text: outcome.text }], isError: outcome.isError });
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
    log("usage: staged-note-mcp.cjs <staged-session-id> <turn-id>");
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
  process.stdout.on("error", function (error) {
    if (error && error.code === "EPIPE") process.exit(0);
    throw error;
  });
  const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  lines.on("line", function (line) {
    handleLine(sessionId, turnId, line);
  });
  // stdin closing ends the server; the event loop drains stdout on its own.
}

main(process.argv.slice(2));
