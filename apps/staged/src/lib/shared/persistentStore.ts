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

// ---------------------------------------------------------------------------
// Tauri store backend
// ---------------------------------------------------------------------------

interface TauriStoreBackend {
  kind: 'tauri';
  store: import('@tauri-apps/plugin-store').Store;
}

// ---------------------------------------------------------------------------
// localStorage backend (web mode)
// ---------------------------------------------------------------------------

const LOCAL_STORAGE_PREFIX = 'staged:pref:';

interface LocalStorageBackend {
  kind: 'localStorage';
}

// ---------------------------------------------------------------------------
// Singleton
// ---------------------------------------------------------------------------

type StoreBackend = TauriStoreBackend | LocalStorageBackend | null;

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
  } else {
    backend = { kind: 'localStorage' };
  }
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

  if (backend.kind === 'tauri') {
    return backend.store.get<T>(key);
  }

  // localStorage backend
  const raw = localStorage.getItem(LOCAL_STORAGE_PREFIX + key);
  if (raw === null) return undefined;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return undefined;
  }
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

  if (backend.kind === 'tauri') {
    await backend.store.set(key, value);
    return;
  }

  // localStorage backend
  localStorage.setItem(LOCAL_STORAGE_PREFIX + key, JSON.stringify(value));
}

/**
 * Delete a value from the persistent store.
 */
export async function deleteStoreValue(key: string): Promise<void> {
  if (!backend) {
    console.warn('[PersistentStore] Store not initialized, call initPersistentStore() first');
    return;
  }

  if (backend.kind === 'tauri') {
    await backend.store.delete(key);
    return;
  }

  // localStorage backend
  localStorage.removeItem(LOCAL_STORAGE_PREFIX + key);
}
