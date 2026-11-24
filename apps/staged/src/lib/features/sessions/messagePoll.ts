/**
 * Incremental transcript reads for an open session.
 *
 * A `pollSession` fetch re-fetches the last known message (it may still be
 * streaming) plus anything newer. That tail re-fetch is not enough on its own: a mid-message
 * tool call (e.g. one forwarded from a background subagent) inserts tool rows
 * after the assistant row the writer is still extending, so that row falls
 * behind the cursor. The pane asks for it by id alongside the tail.
 *
 * The session stream sends the same rows when they are written instead of
 * when they are asked for (`sessionStream.svelte.ts`), and is what keeps an
 * open pane current. The cursors here are what the pane gives the stream to
 * be read from (`unionSessionPollRequests` when two panes show one session),
 * and a frame's rows are merged by `mergeFrameMessages`. The pane fetches
 * with them itself when it is reopened, when it re-reads a reconciled
 * session with no chat open, and every `FALLBACK_POLL_MS` while the stream
 * is unavailable.
 */

import type { SessionMessage, SessionPollRequest } from '../../types';
import { isToolMetadataSettled } from './acpTranscript';
import { retainInlineToolBodies } from './toolCallBodies';

/**
 * How often an open pane reads `pollSession` while the session stream is
 * unavailable. There is no such tick while the stream is established.
 */
export const FALLBACK_POLL_MS = 1_000;

/**
 * The cursors for one `pollSession` tick, from what the pane has loaded.
 *
 * An empty transcript polls from `0`, which returns every visible row: the
 * same result `mergeSinceMessages([], rows, 0)` turns into the whole list.
 * Metadata rows for unsettled tool calls are mutated in place by the writer
 * rather than re-inserted, so they are re-requested by id alongside the rows
 * past the metadata cursor.
 */
export function sessionPollRequest(
  messages: readonly SessionMessage[],
  acpMetadataMessages: readonly SessionMessage[]
): SessionPollRequest {
  return {
    sinceId: messages.at(-1)?.id ?? 0,
    refetchIds: openAssistantRefetchIds(messages),
    metadataSinceId: acpMetadataMessages.at(-1)?.id ?? 0,
    metadataRefetchIds: acpMetadataMessages
      .filter((message) => message.acpToolCallId && !isToolMetadataSettled(message))
      .map((message) => message.id),
  };
}

/**
 * One cursor for several panes on one session: it reads everything any of
 * them would be sent for its own. The stream reads a session once for a
 * window, and every pane showing it there takes the same frames; rows a pane
 * already holds merge as no-ops.
 */
export function unionSessionPollRequests(
  requests: readonly SessionPollRequest[]
): SessionPollRequest {
  const ids = (pick: (request: SessionPollRequest) => number[]) =>
    [...new Set(requests.flatMap(pick))].sort((a, b) => a - b);
  return {
    sinceId: Math.min(...requests.map((request) => request.sinceId)),
    refetchIds: ids((request) => request.refetchIds),
    metadataSinceId: Math.min(...requests.map((request) => request.metadataSinceId)),
    metadataRefetchIds: ids((request) => request.metadataRefetchIds),
  };
}

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
 * dropped until a next fetch that may never come.
 *
 * That is as far as the merge can go: for a row both the cache and the fetch
 * hold it cannot tell which copy is newer, so it relies on the pane
 * serialising since-fetches (`pollInFlight`) and is not
 * a full defence against out-of-order fetches.
 *
 * A tool row that settled since the last tick comes back with its large
 * bodies deferred; the inline copies already cached are kept where they match
 * ({@link retainInlineToolBodies}) so an expanded card does not refetch them.
 */
export function mergeSinceMessages(
  existing: readonly SessionMessage[],
  updated: readonly SessionMessage[],
  lastId: number
): MergedSinceMessages | null {
  let next: SessionMessage[] | null = null;
  const tail: SessionMessage[] = [];
  for (const update of updated) {
    const index = findIndexFromEnd(existing, update.id);
    const message = index < 0 ? update : retainInlineToolBodies(existing[index], update);
    if (message.id >= lastId) {
      tail.push(message);
      continue;
    }
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

/**
 * Merge the rows of a stream frame into the cached transcript, or return
 * `null` when nothing changed.
 *
 * A frame is not a since-fetch. Its rows are the ones that were written,
 * wherever they sit, and it does not repeat the tail it follows, so nothing
 * can be spliced: each row replaces its cached copy by id (only when it
 * differs, with the same identity rules as `mergeSinceMessages`), and a row
 * the cache does not hold is added in id order. A snapshot is merged the same
 * way; it is read from a cursor, so it does repeat the tail, as a row that
 * replaces itself.
 *
 * Rows older than the first cached row belong to a page that is not loaded
 * (see `transcriptWindow.ts`) and are dropped: `loadOlderMessages` reads them
 * when the user gets there. That is also what keeps a snapshot read from
 * another pane's cursor from growing this pane's window backwards.
 *
 * Unlike a since-fetch, a frame says nothing about rows it does not carry, so
 * nothing is ever removed here.
 */
export function mergeFrameMessages(
  existing: readonly SessionMessage[],
  updated: readonly SessionMessage[]
): MergedSinceMessages | null {
  const firstId = existing[0]?.id;
  const lastId = existing[existing.length - 1]?.id;
  let next: SessionMessage[] | null = null;
  const added = new Map<number, SessionMessage>();
  for (const update of updated) {
    const index = findIndexFromEnd(existing, update.id);
    if (index < 0) {
      if (firstId === undefined || update.id > firstId) added.set(update.id, update);
      continue;
    }
    const message = retainInlineToolBodies(existing[index], update);
    if (isSameMessage(existing[index], message)) continue;
    next ??= [...existing];
    next[index] = message;
  }
  if (added.size === 0) return next ? { messages: next, appended: false } : null;

  const rows = [...added.values()].sort((a, b) => a.id - b.id);
  const base = next ?? existing;
  const newestAdded = rows[rows.length - 1].id;
  if (lastId === undefined || rows[0].id > lastId) {
    return { messages: [...base, ...rows], appended: true };
  }
  // A row inside the cached span that the cache does not hold: it joins in
  // id order.
  return {
    messages: [...base, ...rows].sort((a, b) => a.id - b.id),
    appended: newestAdded > lastId,
  };
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
