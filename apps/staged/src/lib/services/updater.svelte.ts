/**
 * Reactive wiring for the stable-channel updater service.
 *
 * Exposes a shared `$state` record (`updaterState`) that the app shell and a
 * future Settings surface can observe, plus lifecycle helpers for App.svelte.
 * All orchestration lives in the rune-free `./updater` module so it can be
 * unit-tested without a Svelte compile step or native plugins.
 */

import {
  createUpdaterService,
  initialUpdaterState,
  type UpdaterService,
  type UpdaterState,
} from './updater';

/** Shared reactive updater status for the app shell and Settings UI. */
export const updaterState: UpdaterState = $state(initialUpdaterState());

let service: UpdaterService | null = null;

/**
 * Initialize the updater service (idempotent). Applies availability gating,
 * runs the startup check, and schedules the periodic background checks.
 * Returns a cleanup function for `onDestroy`.
 */
export function initUpdater(): () => void {
  if (!service) {
    service = createUpdaterService({ state: updaterState });
    service.start();
  }
  return disposeUpdater;
}

/** Explicit manual update check (Settings entry point). */
export async function checkForUpdatesNow(): Promise<void> {
  await service?.checkNow();
}

/** Stop periodic checks and cancel in-flight updater work. */
export function disposeUpdater(): void {
  service?.dispose();
  service = null;
}
