/**
 * sq CLI availability cache.
 *
 * Checked once at app startup and cached here so components can
 * read the result synchronously without introducing a per-open delay.
 */

import { isSqAvailable } from '../../commands';

/** Shared reactive state for sq CLI availability. */
export const sqState = $state({
  available: false,
  loaded: false,
});

/**
 * Detect whether the `sq` CLI is present and update the cache.
 *
 * Called once at startup from App.svelte. Safe to call again if needed.
 */
export async function refreshSqAvailability(): Promise<boolean> {
  try {
    const available = await isSqAvailable();
    sqState.available = available;
    sqState.loaded = true;
    return available;
  } catch {
    sqState.available = false;
    sqState.loaded = true;
    return false;
  }
}
