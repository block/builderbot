/**
 * Decisions a SessionChatPane makes when it is shown again rather than built
 * fresh. Panes outlive their dialogs (see `sessionPanePool.svelte.ts`), so a
 * reopen can find the transcript already rendered.
 */

import { anchoredScrollTop, type ScrollAnchor } from './scrollAnchor';

export type ReopenMode = 'revalidate' | 'full';

/**
 * How `loadSession` should bring the pane up to date.
 *
 * `revalidate` keeps the rendered transcript and asks for one `poll_session`
 * delta: the same session is loaded, it has rows to extend, and the last load
 * did not end in an error. Anything else — a different session, a transcript
 * that never loaded, or an error such as "Session not found" — needs the
 * `full` load, which replaces everything behind the loading state.
 */
export function reopenMode(state: {
  loadedSessionId: string | null | undefined;
  sessionId: string;
  messageCount: number;
  error: string | null;
}): ReopenMode {
  if (state.loadedSessionId !== state.sessionId) return 'full';
  if (state.messageCount === 0) return 'full';
  if (state.error !== null) return 'full';
  return 'revalidate';
}

/**
 * What one `poll_session` delta did for a reopen. `applied` merged a live
 * session; `missing` is a delta for a session the store no longer has
 * (`session: null`), `failed` a rejected request, `skipped` a poll that never
 * ran (the pane closed, or had no session to extend).
 */
export type PollOutcome = 'applied' | 'missing' | 'failed' | 'skipped';

/**
 * Whether a reopen that tried to revalidate is done, or must run the full
 * load after all. Only an applied delta leaves the pane current: a session
 * deleted or errored while the pane was parked keeps its stale transcript
 * through the delta, and the full load is what turns that into the error
 * state ("Session not found") instead of a poll loop over a dead session.
 */
export function reopenAfterRevalidate(outcome: PollOutcome): 'settled' | 'full' {
  return outcome === 'applied' ? 'settled' : 'full';
}

export interface ParkedScroll {
  /** `scrollTop` of the transcript when the pane left the document. */
  scrollTop: number;
  /** Auto-scroll was on, so the reopen pins to the bottom whatever grew. */
  atBottom: boolean;
  /** The first group in view and its offset, for a box that reflowed. */
  anchor: ScrollAnchor | null;
}

/**
 * Where the transcript scrolls to when its host is reattached. WebKit drops
 * `scrollTop` on detach, so the position is restored explicitly: a pane that
 * was following the bottom goes back to the bottom; otherwise the anchored
 * group goes back to its offset when it is still rendered (`anchorTop` is
 * where it sits now relative to the box's top, after the restore's layout),
 * which holds across a box of a different width; and failing both the saved
 * offset is restored, clamped to what the transcript can reach (rows trimmed
 * while parked).
 */
export function restoredScrollTop(
  saved: ParkedScroll,
  box: { scrollTop: number; scrollHeight: number; clientHeight: number },
  anchorTop: number | null
): number {
  const max = Math.max(0, box.scrollHeight - box.clientHeight);
  if (saved.atBottom) return max;
  const target =
    saved.anchor && anchorTop !== null
      ? anchoredScrollTop(box.scrollTop, saved.anchor, anchorTop)
      : saved.scrollTop;
  return Math.min(Math.max(0, target), max);
}
