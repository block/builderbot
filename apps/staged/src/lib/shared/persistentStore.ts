/**
 * Persistent Store Service
 *
 * Wraps Tauri's store plugin to provide persistent key-value storage
 * that works reliably across dev server restarts (unlike localStorage
 * which is origin-scoped and breaks when the dev port changes).
 *
 * In web mode, falls back to localStorage with JSON serialization.
 *
 * The store is saved to `~/.staged/preferences.json` in Tauri mode.
 */

import { isTauri, invokeCommand } from '../transport';

/** Log a `persistentStore.set` whose round-trip takes at least this long. */
const SLOW_SET_MS = 50;

// ---------------------------------------------------------------------------
// Tauri store backend
// ---------------------------------------------------------------------------

interface TauriStoreBackend {
  kind: 'tauri';
  store: import('@tauri-apps/plugin-store').Store;
}

// ---------------------------------------------------------------------------
// localStorage backend (web mode) — stubbed out
// TODO(web): restore localStorage backend from the `mobile-web` branch
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Singleton
// ---------------------------------------------------------------------------

type StoreBackend = TauriStoreBackend | null;

let backend: StoreBackend = null;

/**
 * Initialize the persistent store.
 * Must be called once at app startup before using get/set.
 */
export async function initPersistentStore(): Promise<void> {
  if (backend) return;

  if (isTauri) {
    const storePath = await invokeCommand<string>('preferences_store_path');
    const { load } = await import('@tauri-apps/plugin-store');
    const store = await load(storePath, {
      defaults: {},
      autoSave: true,
      overrideDefaults: true,
    });
    backend = { kind: 'tauri', store };
  }
  // TODO(web): restore localStorage backend initialization for web mode
}

/**
 * Get a value from the persistent store.
 * Returns undefined if the key doesn't exist.
 */
export async function getStoreValue<T>(key: string): Promise<T | undefined> {
  if (!backend) {
    console.warn('[PersistentStore] Store not initialized, call initPersistentStore() first');
    return undefined;
  }

  return backend.store.get<T>(key);
}

/**
 * Set a value in the persistent store.
 * The value is automatically persisted to disk.
 */
export async function setStoreValue<T>(key: string, value: T): Promise<void> {
  if (!backend) {
    console.warn('[PersistentStore] Store not initialized, call initPersistentStore() first');
    return;
  }

  // Concurrent event-loop probe. `store.set` is an IPC round-trip whose promise
  // can only resolve once the renderer's main thread is free to process the
  // response — so a slow `set` reading does NOT prove the disk write was slow,
  // it may just be queued behind a blocked main thread. This macrotask is
  // scheduled at the same instant and is delayed by exactly that same block, so
  // comparing the two attributes the cost: `set` >> `lag` => genuinely slow
  // backend write/IPC; `set` ≈ `lag` => the write is a co-victim of a
  // main-thread freeze (look at switchTracer's `maxGap` for the culprit).
  let lagMs = -1;
  const probeStart = performance.now();
  const lagProbe = new Promise<void>((resolve) => {
    setTimeout(() => {
      lagMs = performance.now() - probeStart;
      resolve();
    }, 0);
  });

  const start = performance.now();
  await backend.store.set(key, value);
  const dur = performance.now() - start;
  await lagProbe;
  if (dur >= SLOW_SET_MS) {
    console.info(
      `[switch] persistentStore.set slow: key=${key} set=${Math.round(dur)}ms eventLoopLag=${Math.round(lagMs)}ms`
    );
  }
}

/**
 * Delete a value from the persistent store.
 */
export async function deleteStoreValue(key: string): Promise<void> {
  if (!backend) {
    console.warn('[PersistentStore] Store not initialized, call initPersistentStore() first');
    return;
  }

  await backend.store.delete(key);
}
