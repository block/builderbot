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
