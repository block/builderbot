/**
 * Incremental transcript polling for a running session.
 *
 * Each tick re-fetches the last known message (it may still be streaming) plus
 * anything newer. That tail re-fetch is not enough on its own: a mid-message
 * tool call (e.g. one forwarded from a background subagent) inserts tool rows
 * after the assistant row the writer is still extending, so that row falls
 * behind the cursor. The pane asks for it by id alongside the tail.
 */

import type { SessionMessage } from '../../types';

/**
 * Ids of rows before the tail the writer may still be extending: the most
 * recent assistant row, when later rows have pushed it off the tail.
 *
 * The writer only opens an assistant row when none is open, and tool rows
 * never open one, so a still-open row is always the latest assistant row. The
 * transcript carries no "open" marker, so a closed one is re-fetched too —
 * one small row per tick while the session runs.
 */
export function openAssistantRefetchIds(messages: readonly SessionMessage[]): number[] {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role !== 'assistant') continue;
    return i === messages.length - 1 ? [] : [messages[i].id];
  }
  return [];
}

export interface MergedSinceMessages {
  messages: SessionMessage[];
  /** Whether a row past the previous tail arrived (the scroll condition). */
  appended: boolean;
}

/**
 * Merge a since-fetch into the cached transcript, or return `null` when
 * nothing changed.
 *
 * Rows before `lastId` replace their cached copy only when it differs, and the
 * transcript array is only rebuilt when something did: reassigning the same
 * data under a fresh identity would invalidate the grouped transcript and
 * re-render the entire message list on every tick. Rows from `lastId` on
 * replace the tail.
 */
export function mergeSinceMessages(
  existing: readonly SessionMessage[],
  updated: readonly SessionMessage[],
  lastId: number
): MergedSinceMessages | null {
  let next: SessionMessage[] | null = null;
  const tail: SessionMessage[] = [];
  for (const message of updated) {
    if (message.id >= lastId) {
      tail.push(message);
      continue;
    }
    const index = findIndexFromEnd(existing, message.id);
    if (index < 0 || isSameMessage(existing[index], message)) continue;
    next ??= [...existing];
    next[index] = message;
  }

  let appended = false;
  if (tail.length > 0 && !isUnchangedTail(existing, tail, lastId)) {
    next = [...(next ?? existing).slice(0, -1), ...tail];
    appended = tail.length > 1 || tail[0].id !== lastId;
  }

  return next ? { messages: next, appended } : null;
}

/** Whether the tail re-fetch returned only an identical copy of the last
 *  known message. */
function isUnchangedTail(
  existing: readonly SessionMessage[],
  tail: readonly SessionMessage[],
  lastId: number
): boolean {
  if (tail.length !== 1 || tail[0].id !== lastId) return false;
  return isSameMessage(existing[existing.length - 1], tail[0]);
}

/** Content is compared separately so the common streaming case (the row
 *  grew) skips the JSON pass over the metadata fields. */
function isSameMessage(current: SessionMessage, update: SessionMessage): boolean {
  if (update.content !== current.content) return false;
  return JSON.stringify({ ...update, content: '' }) === JSON.stringify({ ...current, content: '' });
}

/** Refetched rows sit near the end of the transcript, so search backwards. */
function findIndexFromEnd(messages: readonly SessionMessage[], id: number): number {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].id === id) return i;
  }
  return -1;
}
