import type { AcpTranscriptGroup } from './acpTranscript';
import { writeNoteToolCall } from './noteToolCall';

/**
 * The closing message a note session is asked to send once `write_note` has
 * saved its note. The tool result asks for this one word so the turn ends
 * without a recap of the note; the chat pane hides the message, since the
 * "Saved note" row above it already says what happened. The row itself stays
 * stored: hiding is a rendering decision, so a wrong match loses nothing.
 */
export const NOTE_DONE_WORD = 'Done';

/** Wrappers an agent may put around the word: Markdown emphasis, code, quotes. */
const WRAPPERS: ReadonlyArray<readonly [open: string, close: string]> = [
  // `**` before `*`, so bold is taken off whole rather than as one asterisk.
  ['**', '**'],
  ['*', '*'],
  ['_', '_'],
  ['`', '`'],
  ['"', '"'],
  ["'", "'"],
  ['“', '”'],
  ['‘', '’'],
];

/**
 * Whether an assistant message's text is nothing but the closing word:
 * `Done`, in any case, optionally in one pair of wrappers and with one
 * trailing `.` or `!`. Anything more is real text: `Done ✅`, a sentence that
 * starts with the word, an answer whose last line is `Done`.
 */
export function isNoteDoneText(text: string | null | undefined): boolean {
  if (typeof text !== 'string') return false;
  let word = text.trim();
  for (const [open, close] of WRAPPERS) {
    if (
      word.length >= open.length + close.length &&
      word.startsWith(open) &&
      word.endsWith(close)
    ) {
      word = word.slice(open.length, word.length - close.length);
      break;
    }
  }
  if (word.endsWith('.') || word.endsWith('!')) word = word.slice(0, -1);
  return word.toLowerCase() === NOTE_DONE_WORD.toLowerCase();
}

/**
 * The ids of the assistant messages the chat pane hides: the bare "Done" that
 * closes a turn after a saved `write_note` call.
 *
 * A message is hidden only when all of these hold:
 *
 * 1. A saved call comes directly before it: the nearest earlier group is a
 *    tool group whose last `write_note` item resolved to `completed`. Other
 *    items in that group do not matter. Codex completes a review item after
 *    the call and Claude sometimes issues a shell call beside it.
 * 2. Every assistant message between that tool group and the end of the turn
 *    is a bare "Done" (see `isNoteDoneText`). A background task that wakes
 *    the session can add a second one, and both are hidden. A recap after
 *    the first shows both.
 * 3. The turn ends there: what follows is the next user message or the end
 *    of the transcript, not a tool call.
 *
 * `acp` groups render nothing and are stepped over throughout.
 *
 * The test is keyed on the call, so no other message can match, and it fails
 * open: on any mismatch the text is shown whole, exactly as written.
 */
export function hiddenNoteDoneMessageIds(groups: readonly AcpTranscriptGroup[]): Set<number> {
  const hidden = new Set<number>();
  for (let index = 0; index < groups.length; index++) {
    const group = groups[index];
    if (group.type !== 'tools' || !endsWithSavedNote(group)) continue;

    const run: number[] = [];
    let next = index + 1;
    let allDone = true;
    for (; next < groups.length; next++) {
      const follower = groups[next];
      if (follower.type === 'acp') continue;
      if (follower.type !== 'assistant') break;
      if (!isNoteDoneText(follower.message.content)) allDone = false;
      run.push(follower.message.id);
    }
    const endsTurn = next >= groups.length || groups[next].type === 'user';
    if (allDone && endsTurn) {
      for (const id of run) hidden.add(id);
    }
  }
  return hidden;
}

/** Whether the last `write_note` item of a tool group is a saved call. */
function endsWithSavedNote(group: Extract<AcpTranscriptGroup, { type: 'tools' }>): boolean {
  for (let index = group.items.length - 1; index >= 0; index--) {
    const item = group.items[index];
    if (writeNoteToolCall(item.call.content, item.rawInput)) return item.status === 'completed';
  }
  return false;
}
