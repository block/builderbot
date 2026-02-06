/**
 * Persistent Store Service
 *
 * Wraps Tauri's store plugin to provide persistent key-value storage
 * that works reliably across dev server restarts (unlike localStorage
 * which is origin-scoped and breaks when the dev port changes).
 *
 * The store is saved to the app's data directory as a JSON file.
 */

import { load, type Store } from '@tauri-apps/plugin-store';

// Singleton store instance
let store: Store | null = null;

// Track active key change listeners for cleanup
const keyChangeUnlisteners: Map<string, () => void> = new Map();

/**
 * Initialize the persistent store.
 * Must be called once at app startup before using get/set.
 */
export async function initPersistentStore(): Promise<void> {
  if (store) return;

  // Load or create the store file in the app data directory
  // The file is automatically saved when values change
  // We use overrideDefaults to load existing values from disk
  store = await load('preferences.json', {
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

/**
 * Check if the store has been initialized.
 */
export function isStoreInitialized(): boolean {
  return store !== null;
}

/**
 * Subscribe to changes for a specific key.
 * The callback is called whenever the key's value changes.
 * Returns an unsubscribe function.
 */
export async function onKeyChange<T>(
  key: string,
  callback: (value: T | undefined) => void
): Promise<() => void> {
  if (!store) {
    console.warn('[PersistentStore] Store not initialized, call initPersistentStore() first');
    return () => {};
  }

  // Remove existing listener for this key if any
  const existingUnlisten = keyChangeUnlisteners.get(key);
  if (existingUnlisten) {
    existingUnlisten();
    keyChangeUnlisteners.delete(key);
  }

  // Subscribe to changes
  const unlisten = await store.onKeyChange<T>(key, callback);

  // Track for cleanup
  keyChangeUnlisteners.set(key, unlisten);

  return () => {
    unlisten();
    keyChangeUnlisteners.delete(key);
  };
}
