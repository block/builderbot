import type { ToolStatus } from './acpTranscript';
import { parseToolCall } from './sessionModalHelpers';

/**
 * The `write_note` call a note session makes on Staged's `staged` MCP server
 * (see `docs/note-delivery.md`). The transcript row carries the tool's
 * arguments, so the chat pane can show the note the agent saved instead of
 * a shell-styled `$ mcp__staged__write_note` line.
 */
export interface WriteNoteCall {
  title: string;
  body: string;
  suggestedNextCommitStep: string | null;
  suggestedNextNoteStep: string | null;
}

/**
 * Whether a tool name is the `staged` server's `write_note`, in any of the
 * spellings the bridges use: Claude Code's `mcp__staged__write_note`,
 * Goose's `staged: write note`, or a bare `staged.write_note`.
 */
export function isWriteNoteToolName(name: string | null | undefined): boolean {
  if (typeof name !== 'string') return false;
  const normalized = name
    .trim()
    .toLowerCase()
    .replace(/[\s-]+/g, '_')
    .replace(/^mcp__/, '');
  return /^staged(?:__|:_|_|\.)write_note$/.test(normalized);
}

/** The note arguments from a `write_note` input object, if it has them. */
export function writeNoteCallFromInput(input: unknown): WriteNoteCall | null {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
  const record = input as Record<string, unknown>;
  const title = typeof record.title === 'string' ? record.title.trim() : '';
  const body = typeof record.body === 'string' ? record.body : '';
  if (!title && !body) return null;
  const steps = record.suggested_next_steps ?? record.suggestedNextSteps;
  const stepRecord =
    steps && typeof steps === 'object' && !Array.isArray(steps)
      ? (steps as Record<string, unknown>)
      : null;
  return {
    title,
    body,
    suggestedNextCommitStep: stepString(
      stepRecord?.suggestedNextCommitStep ?? stepRecord?.suggested_next_commit_step
    ),
    suggestedNextNoteStep: stepString(
      stepRecord?.suggestedNextNoteStep ?? stepRecord?.suggested_next_note_step
    ),
  };
}

function stepString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

/**
 * The `write_note` call behind a tool row, or `null` for any other tool.
 * `content` is the `tool_call` row's content (JSON `{name, input}` from the
 * bridges, or plain text); `rawInput` is the ACP `rawInput`, preferred when
 * present since it is the arguments verbatim.
 */
export function writeNoteToolCall(content: string, rawInput: unknown): WriteNoteCall | null {
  const parsed = parseToolCall(content);
  const name = parsed?.name ?? content;
  if (!isWriteNoteToolName(name)) return null;
  return (
    writeNoteCallFromInput(rawInput) ??
    writeNoteCallFromInput(parsed?.args) ?? {
      title: '',
      body: '',
      suggestedNextCommitStep: null,
      suggestedNextNoteStep: null,
    }
  );
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
