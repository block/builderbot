/**
 * Reactive wiring for the stable-channel updater service.
 *
 * Exposes a shared `$state` record (`updaterState`) that the app shell and the
 * Settings surface observe, plus lifecycle helpers for App.svelte. All
 * orchestration lives in the rune-free `./updater` and `./updaterBridge`
 * modules so it can be unit-tested without a Svelte compile step or native
 * plugins.
 *
 * Each window has its own copy of this module. The window that owns the
 * updater runs the service (`initUpdater`); every other window runs a mirror
 * (`initUpdaterMirror`) that reflects the owner's state and routes manual
 * checks to it.
 */

import {
  createUpdaterService,
  initialUpdaterState,
  type UpdaterService,
  type UpdaterState,
} from './updater';
import {
  createUpdaterMirror,
  createUpdaterOwnerBridge,
  type UpdaterMirror,
  type UpdaterOwnerBridge,
} from './updaterBridge';

/** Shared reactive updater status for the app shell and Settings UI. */
export const updaterState: UpdaterState = $state(initialUpdaterState());

let service: UpdaterService | null = null;
let ownerBridge: UpdaterOwnerBridge | null = null;
let mirror: UpdaterMirror | null = null;

function disposeMirror(): void {
  mirror?.dispose();
  mirror = null;
}

/**
 * Initialize the updater service in the owning window (idempotent). Applies
 * availability gating, runs the startup check, schedules the periodic
 * background checks, and broadcasts every state change to other windows.
 * Returns a cleanup function for `onDestroy`.
 */
export function initUpdater(): () => void {
  if (!service) {
    // A window that wins ownership after mirroring must stop applying the old
    // owner's state, and must not carry its fields into the new service.
    disposeMirror();
    Object.assign(updaterState, initialUpdaterState());
    service = createUpdaterService({
      state: updaterState,
      onStateChange: () => ownerBridge?.broadcast(),
    });
    ownerBridge = createUpdaterOwnerBridge({
      state: updaterState,
      checkNow: () => service?.checkNow() ?? Promise.resolve(),
    });
    service.start();
  }
  return disposeUpdater;
}

/**
 * Mirror the owning window's updater state into this window (for windows that
 * lost the ownership claim). Replaces any existing mirror. Returns a cleanup
 * function that disposes only the mirror it created.
 */
export function initUpdaterMirror(): () => void {
  disposeMirror();
  const created = createUpdaterMirror({ state: updaterState });
  mirror = created;
  return () => {
    if (mirror !== created) return;
    disposeMirror();
  };
}

/**
 * Explicit manual update check (Settings entry point). In a mirror window the
 * request is routed to the owner; progress arrives through the synced state.
 */
export async function checkForUpdatesNow(): Promise<void> {
  if (service) await service.checkNow();
  else mirror?.requestCheck();
}

/** Stop periodic checks and cancel in-flight updater work. */
export function disposeUpdater(): void {
  service?.dispose();
  service = null;
  ownerBridge?.dispose();
  ownerBridge = null;
}
