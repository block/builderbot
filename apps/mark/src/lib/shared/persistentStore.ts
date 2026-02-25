/**
 * Persistent Store Service
 *
 * Wraps Tauri's store plugin to provide persistent key-value storage
 * that works reliably across dev server restarts (unlike localStorage
 * which is origin-scoped and breaks when the dev port changes).
 *
 * The store is saved to `~/.mark/preferences.json`.
 */

import { invoke } from '@tauri-apps/api/core';
import { load, type Store } from '@tauri-apps/plugin-store';

// Singleton store instance
let store: Store | null = null;

async function getPreferencesStorePath(): Promise<string> {
  return invoke<string>('preferences_store_path');
}

/**
 * Initialize the persistent store.
 * Must be called once at app startup before using get/set.
 */
export async function initPersistentStore(): Promise<void> {
  if (store) return;

  const storePath = await getPreferencesStorePath();
  store = await load(storePath, {
    defaults: {},
    autoSave: true,
    overrideDefaults: true,
  });
}

/**
 * Get a value from the persistent store.
 * Returns undefined if the key doesn't exist.
 */
export async function getStoreValue<T>(key: string): Promise<T | undefined> {
  if (!store) {
    console.warn('[PersistentStore] Store not initialized, call initPersistentStore() first');
    return undefined;
  }

  return store.get<T>(key);
}

/**
 * Set a value in the persistent store.
 * The value is automatically persisted to disk.
 */
export async function setStoreValue<T>(key: string, value: T): Promise<void> {
  if (!store) {
    console.warn('[PersistentStore] Store not initialized, call initPersistentStore() first');
    return;
  }

  await store.set(key, value);
}

/**
 * Delete a value from the persistent store.
 */
export async function deleteStoreValue(key: string): Promise<void> {
  if (!store) {
    console.warn('[PersistentStore] Store not initialized, call initPersistentStore() first');
    return;
  }

  await store.delete(key);
}
