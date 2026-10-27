/**
 * Decisions a SessionChatPane makes when it is shown again rather than built
 * fresh. Panes outlive their dialogs (see `sessionPanePool.svelte.ts`), so a
 * reopen can find the transcript already rendered.
 */

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

export interface ParkedScroll {
  /** `scrollTop` of the transcript when the pane left the document. */
  scrollTop: number;
  /** Auto-scroll was on, so the reopen pins to the bottom whatever grew. */
  atBottom: boolean;
}

/**
 * Where the transcript scrolls to when its host is reattached. WebKit drops
 * `scrollTop` on detach, so the saved offset is restored explicitly; a pane
 * that was following the bottom goes back to the bottom, and an offset the
 * transcript can no longer reach (rows trimmed while parked) is clamped.
 */
export function restoredScrollTop(
  saved: ParkedScroll,
  scrollHeight: number,
  clientHeight: number
): number {
  const max = Math.max(0, scrollHeight - clientHeight);
  if (saved.atBottom) return max;
  return Math.min(Math.max(0, saved.scrollTop), max);
}
