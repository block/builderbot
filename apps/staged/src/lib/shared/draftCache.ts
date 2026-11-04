import { deleteStoreValue, getStoreValue, listStoreKeys, setStoreValue } from './persistentStore';

export type DraftEntry = {
  value: string;
  updatedAt: number;
  expiresAt: number;
};

/** Namespace every draft key shares, so expired entries can be swept. */
export const DRAFT_KEY_PREFIX = 'draft:';
export const DRAFT_TTL_MS = 7 * 24 * 60 * 60 * 1000;
export const DRAFT_WRITE_DEBOUNCE_MS = 350;

function isDraftEntry(value: unknown): value is DraftEntry {
  if (!value || typeof value !== 'object') return false;
  const entry = value as Partial<DraftEntry>;
  return (
    typeof entry.value === 'string' &&
    typeof entry.updatedAt === 'number' &&
    typeof entry.expiresAt === 'number'
  );
}

export async function loadDraft(key: string, now = Date.now()): Promise<string | null> {
  const entry = await getStoreValue<unknown>(key);
  if (!isDraftEntry(entry)) return null;

  if (entry.expiresAt <= now) {
    await deleteStoreValue(key);
    return null;
  }

  return entry.value;
}

export async function saveDraft(key: string, value: string, now = Date.now()): Promise<void> {
  if (value.length === 0) {
    await deleteStoreValue(key);
    return;
  }

  await setStoreValue<DraftEntry>(key, {
    value,
    updatedAt: now,
    expiresAt: now + DRAFT_TTL_MS,
  });
}

export async function clearDraft(key: string): Promise<void> {
  await deleteStoreValue(key);
}

/**
 * Delete every expired `draft:*` entry from the store.
 *
 * `loadDraft` only expires the one key it reads, and draft keys are scoped to a
 * session/branch/project id — so once the owner is deleted its key is never
 * read again and the user's text would sit in `~/.staged/preferences.json`
 * forever, next to real preferences. Sweeping at startup bounds that to the
 * TTL. Entries under the namespace that no longer parse as drafts are dropped
 * too; `loadDraft` already treats them as absent.
 *
 * Returns the number of keys deleted.
 */
export async function sweepExpiredDrafts(now = Date.now()): Promise<number> {
  const keys = (await listStoreKeys()).filter((key) => key.startsWith(DRAFT_KEY_PREFIX));

  let deleted = 0;
  for (const key of keys) {
    const entry = await getStoreValue<unknown>(key);
    if (isDraftEntry(entry) && entry.expiresAt > now) continue;
    await deleteStoreValue(key);
    deleted += 1;
  }

  return deleted;
}

export function createDraftWriteScheduler(delayMs = DRAFT_WRITE_DEBOUNCE_MS) {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pendingKey: string | null = null;
  let pendingValue = '';
  let writeQueue: Promise<void> = Promise.resolve();

  function enqueue(write: () => Promise<void>): Promise<void> {
    // The queue's promises never reject: several call sites fire-and-forget
    // (`void` on onDestroy flushes and clearNow), so a rejected write would
    // surface as an unhandled rejection at teardown and be invisible
    // mid-session. Log it here instead and keep the chain alive.
    writeQueue = writeQueue.then(write).catch((err) => {
      console.error('[draftCache] Failed to persist draft:', err);
    });
    return writeQueue;
  }

  function cancelPending() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    pendingKey = null;
  }

  function flushPending(): Promise<void> {
    if (pendingKey === null) return writeQueue;
    const key = pendingKey;
    const value = pendingValue;
    cancelPending();
    return enqueue(() => saveDraft(key, value));
  }

  // One scheduler instance can be reused across targets (e.g. the session
  // modal keeps one writer while switching sessions). A pending debounced
  // write for a *different* key must be flushed rather than cancelled, or
  // that key's latest keystrokes would be dropped.
  function settleBeforeWrite(key: string) {
    if (pendingKey !== null && pendingKey !== key) {
      void flushPending();
    } else {
      cancelPending();
    }
  }

  return {
    scheduleSave(key: string, value: string) {
      settleBeforeWrite(key);
      pendingKey = key;
      pendingValue = value;
      timer = setTimeout(() => {
        timer = null;
        pendingKey = null;
        void enqueue(() => saveDraft(key, value));
      }, delayMs);
    },
    saveNow(key: string, value: string): Promise<void> {
      settleBeforeWrite(key);
      return enqueue(() => saveDraft(key, value));
    },
    clearNow(key: string): Promise<void> {
      settleBeforeWrite(key);
      return enqueue(() => clearDraft(key));
    },
    flush: flushPending,
  };
}
