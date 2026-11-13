/**
 * The reveal of this window, and the word to the backend that it happened.
 *
 * Every window is created hidden, so the first thing the user sees is a
 * themed page and not an unpainted webview. The frontend shows the window
 * itself once the theme is applied. The backend cannot see that happen: a
 * window's visibility does not say who showed it, and a window that was
 * revealed can be hidden again. So the frontend reports it.
 *
 * The report matters to `stagedctl project open`. A request that lands while
 * the window is still loading is not allowed to show it early; the backend
 * owes the window a raise instead, and makes it when this report arrives.
 * Once the report is in, a later request raises the window at once, even one
 * that has never held focus and is hidden at the time.
 */

import { noteWindowRevealed } from '../commands';
import { getWindowSync, isTauri } from '../transport';

/**
 * Show this window, then tell the backend it is revealed.
 *
 * Rejects only when the window could not be shown, as the bare `show()` did.
 * A report that fails is logged and dropped: the window is on screen, which
 * is what the caller asked for. Web mode has no native window, so nothing is
 * reported there.
 */
export async function revealWindow(): Promise<void> {
  await getWindowSync().show();
  if (!isTauri) return;
  try {
    await noteWindowRevealed();
  } catch (e) {
    console.warn('[windowReveal] Failed to report the window as revealed:', e);
  }
}
