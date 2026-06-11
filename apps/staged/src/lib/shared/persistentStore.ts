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

/** Log a `persistentStore.set` whose disk write blocks for at least this long. */
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

  const start = performance.now();
  await backend.store.set(key, value);
  const dur = performance.now() - start;
  if (dur >= SLOW_SET_MS) {
    console.info(`[switch] persistentStore.set slow: key=${key} took ${Math.round(dur)}ms`);
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
