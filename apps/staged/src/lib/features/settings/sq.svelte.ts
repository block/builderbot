/**
 * sq CLI availability cache.
 *
 * Checked once at app startup and cached here so components can
 * read the result synchronously without introducing a per-open delay.
 */

import { isSqAvailable } from '../../api/commands';

/** Shared reactive state for sq CLI availability. */
export const sqState = $state({
  available: false,
  loaded: false,
});

/**
 * Whether Blox features (remote projects, transfers between local and Blox)
 * can be offered: the `sq` CLI check has landed and found it.
 *
 * False until the startup check resolves, so Blox UI never flashes in for a
 * host without `sq`. Reads `sqState`, so it is reactive in `$derived` and
 * templates. Gate Blox UI on this rather than on `sqState` directly, so the
 * rule lives in one place.
 */
export function bloxAvailable(): boolean {
  return sqState.loaded && sqState.available;
}

let sqAvailabilityPromise: Promise<boolean> | null = null;

async function loadSqAvailability(): Promise<boolean> {
  try {
    const available = await isSqAvailable();
    sqState.available = available;
    sqState.loaded = true;
    return available;
  } catch {
    sqState.available = false;
    sqState.loaded = true;
    return false;
  } finally {
    sqAvailabilityPromise = null;
  }
}

/**
 * Detect whether the `sq` CLI is present and update the cache.
 *
 * Safe to call again if a caller needs to refresh the app-level state.
 */
export async function refreshSqAvailability(): Promise<boolean> {
  sqAvailabilityPromise ??= loadSqAvailability();
  return sqAvailabilityPromise;
}

/**
 * Return cached `sq` availability, loading it once if needed.
 */
export function ensureSqAvailabilityLoaded(): Promise<boolean> {
  if (sqState.loaded) {
    return Promise.resolve(sqState.available);
  }

  return refreshSqAvailability();
}
