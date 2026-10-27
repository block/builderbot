/**
 * Decisions a SessionChatPane makes when it is shown again rather than built
 * fresh. Panes outlive their dialogs (see `sessionPanePool.svelte.ts`), so a
 * reopen can find the transcript already rendered.
 */

import { anchoredScrollTop, anchorTopIn, type ScrollAnchor } from './scrollAnchor';

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

/** What the pane's scroll bookkeeping should read after a restore. */
export interface RestoredScroll {
  /** The box's `scrollTop` after the write (clamped by the box itself). */
  scrollTop: number;
  /** The anchor the restore was made against, to carry forward as current. */
  anchor: ScrollAnchor | null;
}

/**
 * Scroll `el` to where `saved` says the transcript was, and report the
 * position the caller's bookkeeping (`lastScrollTop`, `scrollAnchor` and its
 * capture point) should take, so the scroll handler does not read the restore
 * as the user scrolling up. The anchored group is looked up in the box as laid
 * out now (`lookupAnchorTop`, injectable for tests), only when the pane was
 * not following the bottom. A null `el` — the box is gone or was never bound —
 * is a no-op and returns null: nothing was scrolled, so nothing is recorded.
 */
export function applyParkedScroll(
  el: HTMLElement | null,
  saved: ParkedScroll,
  lookupAnchorTop: (box: HTMLElement, anchor: ScrollAnchor) => number | null = anchorTopIn
): RestoredScroll | null {
  if (!el) return null;
  const anchorTop = saved.anchor && !saved.atBottom ? lookupAnchorTop(el, saved.anchor) : null;
  el.scrollTop = restoredScrollTop(
    saved,
    { scrollTop: el.scrollTop, scrollHeight: el.scrollHeight, clientHeight: el.clientHeight },
    anchorTop
  );
  return { scrollTop: el.scrollTop, anchor: saved.anchor };
}

/** `requestAnimationFrame` / `cancelAnimationFrame`, injectable for tests. */
export interface FrameScheduler {
  request(callback: () => void): number;
  cancel(handle: number): void;
}

const animationFrames: FrameScheduler = {
  request: (callback) => requestAnimationFrame(callback),
  cancel: (handle) => cancelAnimationFrame(handle),
};

/**
 * A scroll restore that waits for the next animation frame.
 *
 * `unpark` runs synchronously inside the adopter's attachment, in the middle
 * of the dialog's mount flush: the host has just been reinserted and more
 * DOM is still to come (the effects behind `active`). Reading `scrollHeight`
 * and writing `scrollTop` there forced a style pass and a layout over the
 * half-built dialog, and everything that mutated afterwards made the frame
 * — and the composer focus before it — pay a second one. A frame callback
 * runs after the flush and before the frame's own style and layout, so the
 * restore's forced layout *is* the frame's and the first paint already shows
 * the restored position.
 *
 * Only ever one restore is pending: a second `schedule` before the frame
 * fires replaces the saved position. `take` hands it back untouched for a
 * pane parked again before the frame (a hidden window never fires one), and
 * `flush` applies it now for a caller that must measure the restored box.
 */
export function createDeferredRestore(
  apply: (saved: ParkedScroll) => void,
  frames: FrameScheduler = animationFrames
) {
  let pending: ParkedScroll | null = null;
  let handle = 0;

  function cancel(): ParkedScroll | null {
    const saved = pending;
    pending = null;
    if (handle) frames.cancel(handle);
    handle = 0;
    return saved;
  }

  return {
    get pending(): boolean {
      return pending !== null;
    },
    schedule(saved: ParkedScroll): void {
      pending = saved;
      if (handle) return;
      handle = frames.request(() => {
        handle = 0;
        const next = pending;
        pending = null;
        if (next) apply(next);
      });
    },
    /** Apply the pending restore now; false when none was pending. */
    flush(): boolean {
      const saved = cancel();
      if (!saved) return false;
      apply(saved);
      return true;
    },
    /** Drop the pending restore and return what it would have applied. */
    take(): ParkedScroll | null {
      return cancel();
    },
  };
}

export type DeferredRestore = ReturnType<typeof createDeferredRestore>;
