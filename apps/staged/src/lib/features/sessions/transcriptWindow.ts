/**
 * Windowed transcript loading for the session pane.
 *
 * The pane opens with the newest rows only and pages backwards as the user
 * scrolls towards the top, so the transcript it holds is a contiguous suffix
 * of the visible rows: everything from `messages[0]` to the tail, with
 * `hasMore` saying whether older rows exist. These helpers keep that
 * invariant across the three ways rows arrive — the opening window (possibly
 * a stale cached copy followed by a fresh one), older pages, and the poll's
 * since-fetch, which only ever touches the tail and is handled by
 * `messagePoll.ts`.
 */

import type { SessionMessage } from '../../types';

/**
 * Visible rows per page. On the two profiled local sessions the newest 300
 * rows are about 50 rendered groups where the transcript alternates prose
 * and tool runs (≈7 rows per group), and 0.7–0.8 MB of JSON before
 * compression against 1.3–1.9 MB for the whole transcript. A row count rather
 * than a group count because long runs of consecutive tool calls collapse
 * into a single group, which would not bound the payload.
 */
export const TRANSCRIPT_PAGE_ROWS = 300;

/** How close to the top (px) the user scrolls before the next page loads. */
export const LOAD_OLDER_THRESHOLD_PX = 600;

export interface TranscriptPage {
  messages: SessionMessage[];
  /** Whether visible rows older than `messages[0]` exist. */
  hasMore: boolean;
}

/** The server does not report whether more rows exist; the client asks for
 *  one row beyond the page it will show and infers it from the count. */
export function pageRequestLimit(pageRows: number): number {
  return pageRows + 1;
}

/**
 * Interpret a response to a `pageRequestLimit(pageRows)` request: a full
 * over-fetch means older rows exist and the extra (oldest) row is dropped so
 * the next page starts exactly where this one ends.
 */
export function transcriptPageFromRows(rows: SessionMessage[], pageRows: number): TranscriptPage {
  if (rows.length > pageRows) {
    return { messages: rows.slice(rows.length - pageRows), hasMore: true };
  }
  return { messages: rows, hasMore: false };
}

/**
 * Prepend an older page. Rows the pane already holds (the page was requested
 * with `beforeId = messages[0].id`, but a reload may have moved the start in
 * the meantime) are skipped so ids stay unique and ascending.
 */
export function prependOlderMessages(
  existing: readonly SessionMessage[],
  older: readonly SessionMessage[]
): SessionMessage[] {
  const startId = existing[0]?.id ?? Number.POSITIVE_INFINITY;
  const fresh = older.filter((message) => message.id < startId);
  return fresh.length === 0 ? [...existing] : [...fresh, ...existing];
}

/**
 * Replace the opening window with a fresh copy of it (the SWR revalidation of
 * a stale cached window). Pages the user has already loaded below the fresh
 * window's start are kept, and then still decide whether more history exists;
 * otherwise the fresh page's own answer stands. Rows the poll appended past
 * the fresh window's end while the revalidation was in flight are kept too.
 */
export function applyLatestWindow(current: TranscriptPage, fresh: TranscriptPage): TranscriptPage {
  const freshStart = fresh.messages[0]?.id;
  if (freshStart === undefined) return fresh;
  const freshEnd = fresh.messages[fresh.messages.length - 1].id;
  const olderPages = current.messages.filter((message) => message.id < freshStart);
  const newerTail = current.messages.filter((message) => message.id > freshEnd);
  return {
    messages: [...olderPages, ...fresh.messages, ...newerTail],
    hasMore: olderPages.length > 0 ? current.hasMore : fresh.hasMore,
  };
}

/**
 * Bound a window that a long-parked pane's catch-up delta has grown without
 * limit. Up to two pages are kept as they are — the poll appends, and the
 * user may have paged back — beyond that only the newest page stays, and the
 * rows dropped from the front are, by construction, older rows that exist on
 * the server, so `hasMore` turns true and `loadOlderMessages` pages them back
 * in from the new start.
 */
export function trimLatestWindow(current: TranscriptPage, pageRows: number): TranscriptPage {
  if (current.messages.length <= 2 * pageRows) return current;
  return { messages: current.messages.slice(current.messages.length - pageRows), hasMore: true };
}

const TOOL_EVENT_KINDS = new Set(['tool_call', 'tool_call_update']);

/**
 * The ACP metadata rows the transcript grouping may see for a window.
 *
 * Metadata is fetched for the whole session (it is a handful of rows), so
 * when the window is partial it can carry rows older than the first loaded
 * visible row: a hidden or in-flight tool row whose visible call is not
 * loaded, or a `pikchr_session_started` announcement for a tool above the
 * window. Grouped as-is, those render as orphan tool cards at the top of the
 * window — possibly stuck "in progress" — or pair an announcement with the
 * wrong tool. They are dropped here; the one exception is a tool row whose
 * call *is* loaded (a hidden row that raced ahead of its visible row at the
 * window boundary), which still carries that tool's input and status.
 *
 * Callers that want the latest of a kind (plan, config options, available
 * commands) keep reading the unfiltered rows: those are session-level, not
 * positional.
 */
export function metadataRowsForWindow(
  acpMetadataMessages: readonly SessionMessage[],
  messages: readonly SessionMessage[],
  hasMore: boolean
): SessionMessage[] {
  const windowStartId = messages[0]?.id;
  if (!hasMore || windowStartId === undefined) return [...acpMetadataMessages];
  const loadedToolCallIds = new Set(
    messages
      .filter((message) => message.role === 'tool_call' && message.acpToolCallId)
      .map((message) => message.acpToolCallId)
  );
  return acpMetadataMessages.filter((row) => {
    if (row.id >= windowStartId) return true;
    return (
      TOOL_EVENT_KINDS.has(row.acpEventKind ?? '') &&
      !!row.acpToolCallId &&
      loadedToolCallIds.has(row.acpToolCallId)
    );
  });
}
