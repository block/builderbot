/**
 * The moments this page asks the backend where its sessions stand.
 *
 * `sessionStatusListener` hydrates its registry from a snapshot at startup
 * and again whenever events may have been missed: a socket reconnect or a
 * long hide in the web build, and in a desktop window, which has neither
 * signal, a return to the window and a slow interval while it is visible.
 * Each of those is also the moment to ask whether anything else pushed to
 * this page has gone quiet for a bad reason, so the listener announces them
 * here and the session stream's owner checks its stream on them.
 *
 * It is its own module so that neither side imports the other.
 */

const listeners = new Set<() => void>();

/** Call `listener` on every hydration from now on. Returns the unsubscribe. */
export function onRehydrate(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** A hydration is starting. */
export function signalRehydrate(): void {
  for (const listener of [...listeners]) {
    try {
      listener();
    } catch (e) {
      console.error('[rehydrateSignal] A listener failed:', e);
    }
  }
}
