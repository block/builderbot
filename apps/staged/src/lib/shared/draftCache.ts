import { deleteStoreValue, getStoreValue, setStoreValue } from './persistentStore';

export type DraftEntry = {
  value: string;
  updatedAt: number;
  expiresAt: number;
};

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

export function createDraftWriteScheduler(delayMs = DRAFT_WRITE_DEBOUNCE_MS) {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pendingKey: string | null = null;
  let pendingValue = '';
  let writeQueue: Promise<void> = Promise.resolve();

  function enqueue(write: () => Promise<void>): Promise<void> {
    writeQueue = writeQueue.catch(() => {}).then(write);
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
