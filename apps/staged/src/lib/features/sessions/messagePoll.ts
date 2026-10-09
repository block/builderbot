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
 *
 * The tail is spliced in at the `lastId` row's current position rather than
 * at the end: `lastId` is read before the fetch awaits, so another poll that
 * overlapped this one may already have extended `existing` past it, and
 * cutting the last row alone would duplicate the rows both fetches returned.
 * When that happens the tail may also end before the cached one (the older
 * fetch landed last); the cached rows past its last id are kept rather than
 * dropped until a next tick that, after the final poll, never comes.
 *
 * That is as far as the merge can go: for a row both the cache and the fetch
 * hold it cannot tell which copy is newer, so it relies on the pane
 * serialising since-fetches (`pollInFlight` / `finalPollPending`) and is not
 * a full defence against out-of-order fetches.
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
  if (tail.length > 0) {
    const cursor = findIndexFromEnd(existing, lastId);
    const cut = cursor < 0 ? Math.max(existing.length - 1, 0) : cursor;
    if (!isUnchangedTail(existing, tail, cut)) {
      const base = next ?? existing;
      const previousTail = existing[existing.length - 1];
      const tailEnd = tail[tail.length - 1].id;
      const kept = base.slice(cut).filter((m) => m.id > tailEnd);
      next = [...base.slice(0, cut), ...tail, ...kept];
      appended = !previousTail || tailEnd > previousTail.id;
    }
  }

  return next ? { messages: next, appended } : null;
}

/** Whether the tail re-fetch returned only identical copies of rows already
 *  cached from `cut` on. A tail shorter than the cached span still counts:
 *  the rows it does not reach are kept, so the result would be unchanged. */
function isUnchangedTail(
  existing: readonly SessionMessage[],
  tail: readonly SessionMessage[],
  cut: number
): boolean {
  if (tail.length > existing.length - cut) return false;
  return tail.every((message, offset) => isSameMessage(existing[cut + offset], message));
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
