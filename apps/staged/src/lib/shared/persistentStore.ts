/**
 * Persistent Store Service
 *
 * Wraps Tauri's store plugin to provide persistent key-value storage
 * that works reliably across dev server restarts (unlike localStorage
 * which is origin-scoped and breaks when the dev port changes).
 *
 * In web mode, proxies get/set/delete to the web server, which persists
 * to the same store file through the same store plugin instance. This
 * keeps server-read preferences (e.g. `branch-prefix`) in sync no matter
 * which client wrote them.
 *
 * The store is saved to `~/.staged/preferences.json` in both modes.
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
// Web backend — preference commands served by the web server
// ---------------------------------------------------------------------------

interface WebStoreBackend {
  kind: 'web';
}

// ---------------------------------------------------------------------------
// Singleton
// ---------------------------------------------------------------------------

type StoreBackend = TauriStoreBackend | WebStoreBackend | null;

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
    backend = { kind: 'web' };
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

  // Missing keys come back as JSON null from the server.
  const value = await invokeCommand<T | null>('get_preference', { key });
  return value ?? undefined;
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

  await invokeCommand('set_preference', { key, value });
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

  await invokeCommand('delete_preference', { key });
}
