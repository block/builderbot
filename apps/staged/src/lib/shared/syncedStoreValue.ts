/**
 * A persistent-store value every client (Tauri window or web tab) keeps in
 * step: any client's write, this one's included, arrives as a store-change
 * event and goes through `apply`, which must normalize missing or malformed
 * values.
 */

import { getStoreValue, listenToStoreKeyChange, setStoreValue } from './persistentStore';

export interface SyncedStoreValue {
  /**
   * Subscribe (once) and load the current value, resolving once a value has
   * landed (or this read failed). It doesn't wait on a newer refresh.
   */
  start(): Promise<void>;
  /** Persist a value the caller has already applied locally. */
  save(value: unknown): void;
  dispose(): void;
}

export function syncedStoreValue(key: string, apply: (value: unknown) => void): SyncedStoreValue {
  let unlisten: (() => void) | undefined;
  let revision = 0;
  // Whether a value has landed (a notification, local edit, or applied read).
  let hasValue = false;

  function receive(value: unknown): void {
    revision += 1;
    hasValue = true;
    apply(value);
  }

  async function refresh(): Promise<void> {
    const current = ++revision;
    try {
      const value = await getStoreValue<unknown>(key);
      // A notification, local edit, or newer refresh wins over an older
      // snapshot. Until one lands, though, the older snapshot is still the
      // freshest value, and a newer refresh overwrites it when it arrives.
      if (current === revision || !hasValue) {
        hasValue = true;
        apply(value);
      }
    } catch (error) {
      console.error(`[PersistentStore] Failed to refresh ${key}:`, error);
    }
  }

  return {
    start(): Promise<void> {
      unlisten ??= listenToStoreKeyChange(key, receive, {
        // Close the initial registration gap and recover missed web events on reconnect.
        onEstablished: () => void refresh(),
      });
      return refresh();
    },
    save(value: unknown): void {
      revision += 1;
      hasValue = true;
      setStoreValue(key, value);
    },
    dispose(): void {
      unlisten?.();
      // No in-flight read applies after this, not even a first one.
      revision += 1;
      hasValue = true;
    },
  };
}
