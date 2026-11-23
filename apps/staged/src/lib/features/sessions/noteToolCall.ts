import type { ToolStatus } from './acpTranscript';
import { parseToolCall } from './sessionModalHelpers';

/**
 * The `write_note` call a note session makes on Staged's `staged` MCP server
 * (see `docs/note-delivery.md`). The call publishes the session's draft file
 * as the note, title and body, so its only argument is the suggested next
 * steps; the chat pane shows those instead of a shell-styled
 * `$ mcp__staged__write_note` line. The title and the body are the stored
 * note's to show.
 */
export interface WriteNoteCall {
  /**
   * The `title` argument, which only a call recorded before the title moved
   * into the draft file carries (or one from a session that still passes
   * it). `null` for every other call: the row then has no name of its own.
   */
  title: string | null;
  suggestedNextCommitStep: string | null;
  suggestedNextNoteStep: string | null;
}

/**
 * Whether a tool name is the `staged` server's `write_note`, in any of the
 * spellings the bridges use: Claude Code's `mcp__staged__write_note`,
 * Goose's `staged: write note`, Codex's `mcp.staged.write_note`, or a bare
 * `staged.write_note`. The `mcp` prefix is optional and the server must be
 * `staged`, so another server's `write_note` is not a note row.
 */
export function isWriteNoteToolName(name: string | null | undefined): boolean {
  if (typeof name !== 'string') return false;
  const normalized = name
    .trim()
    .toLowerCase()
    .replace(/[\s-]+/g, '_')
    .replace(/^mcp(?:__|\.)/, '');
  return /^staged(?:__|:_|_|\.)write_note$/.test(normalized);
}

/**
 * The note arguments from a `write_note` input object, if it has any: the
 * suggested next steps, and the `title` of a call recorded with one. `null`
 * for an input that holds neither, which a call that suggests no step makes.
 * A `body` on a row recorded before the draft file carried the note is
 * ignored.
 */
export function writeNoteCallFromInput(input: unknown): WriteNoteCall | null {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
  const record = input as Record<string, unknown>;
  const title = argumentString(record.title);
  const steps = record.suggested_next_steps ?? record.suggestedNextSteps;
  const stepRecord =
    steps && typeof steps === 'object' && !Array.isArray(steps)
      ? (steps as Record<string, unknown>)
      : null;
  if (title === null && !stepRecord) return null;
  return {
    title,
    suggestedNextCommitStep: argumentString(
      stepRecord?.suggestedNextCommitStep ?? stepRecord?.suggested_next_commit_step
    ),
    suggestedNextNoteStep: argumentString(
      stepRecord?.suggestedNextNoteStep ?? stepRecord?.suggested_next_note_step
    ),
  };
}

/** A trimmed string argument, or `null` when it is missing, blank or not a string. */
function argumentString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

/**
 * The `write_note` call behind a tool row, or `null` for any other tool.
 * `content` is the `tool_call` row's content (JSON `{name, input}` from the
 * bridges, or plain text); `rawInput` is the ACP `rawInput`, preferred when
 * present since it is the arguments verbatim. The tool name alone makes the
 * row a note row: a call needs no argument.
 */
export function writeNoteToolCall(content: string, rawInput: unknown): WriteNoteCall | null {
  const parsed = parseToolCall(content);
  const name = parsed?.name ?? content;
  if (!isWriteNoteToolName(name)) return null;
  return (
    writeNoteCallFromInput(rawInput) ??
    writeNoteCallFromInput(parsed?.args) ?? {
      title: null,
      suggestedNextCommitStep: null,
      suggestedNextNoteStep: null,
    }
  );
}

/**
 * Whether the call has anything of its own for the expanded row to show.
 * The note it published is not the row's to show.
 */
export function writeNoteCallHasDetails(call: WriteNoteCall): boolean {
  return !!(call.title || call.suggestedNextCommitStep || call.suggestedNextNoteStep);
}

/**
 * The text beside a `write_note` row's verb: the title of a call recorded
 * with one, and nothing otherwise, so the row reads as its verb alone.
 */
export function writeNoteRowDetail(call: WriteNoteCall): string {
  return call.title ?? '';
}

/** Header verb for a `write_note` row in each tool status. */
export function writeNoteVerb(status: ToolStatus): string {
  switch (status) {
    case 'completed':
      return 'Saved note';
    case 'failed':
      return 'Note rejected';
    case 'cancelled':
      return 'Note not saved';
    case 'pending':
    case 'in_progress':
      return 'Saving note';
  }
}
