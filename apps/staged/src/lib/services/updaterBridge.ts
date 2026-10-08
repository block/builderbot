/**
 * Cross-window bridge for the stable-channel updater.
 *
 * Exactly one window owns the updater service (the backend's single-owner
 * lease serializes prompts and installs app-wide). Every other window runs a
 * mirror instead: it shows the owner's status and routes manual checks to the
 * owner over frontend events, so its Settings panel stays accurate and usable.
 *
 *   - `updater-state-sync` — owner → all windows, on every state change and in
 *     reply to a state request; payload is the four-field `UpdaterState`.
 *   - `updater-state-request` — mirror → owner, when a mirror starts.
 *   - `updater-check-request` — mirror → owner; runs a manual check.
 *
 * Rune-free with injectable adapters, like `./updater`.
 */

import { listenToEvent, type ListenOptions, type UnlistenFn } from '../transport';
import { logUpdaterMessage, type UpdaterState } from './updater';

export const UPDATER_STATE_SYNC_EVENT = 'updater-state-sync';
export const UPDATER_STATE_REQUEST_EVENT = 'updater-state-request';
export const UPDATER_CHECK_REQUEST_EVENT = 'updater-check-request';

/** Injectable event touchpoints. Defaults use the Tauri event bus. */
export interface UpdaterBridgeAdapters {
  /** Broadcast an event to every window. */
  emit(event: string, payload?: unknown): Promise<void>;
  listen<T>(event: string, callback: (payload: T) => void, opts?: ListenOptions): UnlistenFn;
  log(message: string): void;
}

function defaultAdapters(): UpdaterBridgeAdapters {
  return {
    async emit(event, payload) {
      const { emit } = await import('@tauri-apps/api/event');
      await emit(event, payload);
    },
    listen: listenToEvent,
    log: logUpdaterMessage,
  };
}

function resolveAdapters(adapters?: Partial<UpdaterBridgeAdapters>): UpdaterBridgeAdapters {
  return { ...defaultAdapters(), ...adapters };
}

function emitLogged(adapters: UpdaterBridgeAdapters, event: string, payload?: unknown): void {
  void adapters.emit(event, payload).catch((error) => {
    adapters.log(`[updater] failed to emit ${event}: ${error}`);
  });
}

/** An explicit copy of the state record, so only the four fields cross windows. */
function snapshot(state: UpdaterState): UpdaterState {
  return {
    status: state.status,
    availableVersion: state.availableVersion,
    releaseNotes: state.releaseNotes,
    error: state.error,
  };
}

// =============================================================================
// Owner side
// =============================================================================

export interface UpdaterOwnerBridge {
  /** Send the owner's current state to every window. */
  broadcast(): void;
  dispose(): void;
}

export function createUpdaterOwnerBridge(options: {
  state: UpdaterState;
  checkNow: () => Promise<void>;
  adapters?: Partial<UpdaterBridgeAdapters>;
}): UpdaterOwnerBridge {
  const { state, checkNow } = options;
  const adapters = resolveAdapters(options.adapters);
  let disposed = false;

  function broadcast(): void {
    if (disposed) return;
    emitLogged(adapters, UPDATER_STATE_SYNC_EVENT, snapshot(state));
  }

  const unlistens = [
    adapters.listen<void>(UPDATER_STATE_REQUEST_EVENT, broadcast),
    adapters.listen<void>(UPDATER_CHECK_REQUEST_EVENT, () => {
      if (disposed) return;
      adapters.log('[updater] manual check requested by another window');
      void checkNow();
    }),
  ];

  function dispose(): void {
    if (disposed) return;
    disposed = true;
    for (const unlisten of unlistens) unlisten();
  }

  return { broadcast, dispose };
}

// =============================================================================
// Mirror side
// =============================================================================

export interface UpdaterMirror {
  /** Ask the owning window to run a manual check. */
  requestCheck(): void;
  dispose(): void;
}

export function createUpdaterMirror(options: {
  state: UpdaterState;
  adapters?: Partial<UpdaterBridgeAdapters>;
}): UpdaterMirror {
  const { state } = options;
  const adapters = resolveAdapters(options.adapters);
  let disposed = false;

  // Pre-sync fallback: the updater is active app-wide, so 'idle' ("checked
  // automatically in the background") is accurate even if the owner's reply is
  // missed during an ownership handoff, and it keeps the check button usable.
  Object.assign(state, { status: 'idle', availableVersion: null, releaseNotes: null, error: null });

  const unlisten = adapters.listen<UpdaterState>(
    UPDATER_STATE_SYNC_EVENT,
    (payload) => {
      if (disposed) return;
      Object.assign(state, snapshot(payload));
    },
    {
      // Request the current state only once the sync listener is live, so the
      // owner's reply cannot arrive before this window is listening for it.
      onEstablished: () => {
        if (!disposed) emitLogged(adapters, UPDATER_STATE_REQUEST_EVENT);
      },
    }
  );

  function requestCheck(): void {
    if (disposed) return;
    emitLogged(adapters, UPDATER_CHECK_REQUEST_EVENT);
  }

  function dispose(): void {
    if (disposed) return;
    disposed = true;
    unlisten();
  }

  return { requestCheck, dispose };
}
