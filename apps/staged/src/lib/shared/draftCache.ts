import { deleteStoreValue, getStoreValue, listStoreKeys, setStoreValue } from './persistentStore';

export type DraftEntry = {
  value: string;
  updatedAt: number;
  expiresAt: number;
  /**
   * The draft's own shape, for a composer that holds more than text (a reply
   * with quote blocks). `value` is then its plain-text rendering, so every
   * reader that only knows text — the expiry sweep, an older build — still
   * gets a usable draft. The owner validates it; the cache only carries it.
   */
  structured?: unknown;
};

/** What a draft holds: text, or text plus the structured form it renders. */
export type DraftContent = { value: string; structured?: unknown };

/** Namespace every draft key shares, so expired entries can be swept. */
export const DRAFT_KEY_PREFIX = 'draft:';
export const DRAFT_TTL_MS = 7 * 24 * 60 * 60 * 1000;
export const DRAFT_WRITE_DEBOUNCE_MS = 350;

/**
 * Told about every write this client makes under `draft:*` — the new content,
 * or `null` once the key is gone — so a view can track which drafts exist
 * without reading the store back. Other clients' writes arrive as
 * `store://change` events instead.
 */
type DraftWriteListener = (key: string, content: DraftContent | null) => void;

const writeListeners = new Set<DraftWriteListener>();

export function subscribeDraftWrites(listener: DraftWriteListener): () => void {
  writeListeners.add(listener);
  return () => writeListeners.delete(listener);
}

function notifyDraftWrite(key: string, content: DraftContent | null) {
  for (const listener of writeListeners) listener(key, content);
}

function toDraftContent(content: string | DraftContent): DraftContent {
  return typeof content === 'string' ? { value: content } : content;
}

/** Whether a draft entry holds anything: a non-empty value or a structured form. */
function hasDraftContent(content: DraftContent): boolean {
  return content.value.length > 0 || content.structured !== undefined;
}

export function isDraftEntry(value: unknown): value is DraftEntry {
  if (!value || typeof value !== 'object') return false;
  const entry = value as Partial<DraftEntry>;
  return (
    typeof entry.value === 'string' &&
    typeof entry.updatedAt === 'number' &&
    typeof entry.expiresAt === 'number'
  );
}

export async function loadDraft(key: string, now = Date.now()): Promise<string | null> {
  return (await loadDraftContent(key, now))?.value ?? null;
}

/** Like `loadDraft`, with the structured form when the draft was saved with one. */
export async function loadDraftContent(
  key: string,
  now = Date.now()
): Promise<DraftContent | null> {
  const entry = await getStoreValue<unknown>(key);
  if (!isDraftEntry(entry)) return null;

  if (entry.expiresAt <= now) {
    await deleteStoreValue(key);
    notifyDraftWrite(key, null);
    return null;
  }

  return entry.structured === undefined
    ? { value: entry.value }
    : { value: entry.value, structured: entry.structured };
}

export async function saveDraft(
  key: string,
  content: string | DraftContent,
  now = Date.now()
): Promise<void> {
  const draft = toDraftContent(content);
  if (!hasDraftContent(draft)) {
    await deleteStoreValue(key);
    notifyDraftWrite(key, null);
    return;
  }

  const entry: DraftEntry = {
    value: draft.value,
    updatedAt: now,
    expiresAt: now + DRAFT_TTL_MS,
  };
  if (draft.structured !== undefined) entry.structured = draft.structured;
  await setStoreValue<DraftEntry>(key, entry);
  notifyDraftWrite(key, draft);
}

export async function clearDraft(key: string): Promise<void> {
  await deleteStoreValue(key);
  notifyDraftWrite(key, null);
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
    notifyDraftWrite(key, null);
    deleted += 1;
  }

  return deleted;
}

export function createDraftWriteScheduler(delayMs = DRAFT_WRITE_DEBOUNCE_MS) {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pendingKey: string | null = null;
  let pendingValue: string | DraftContent = '';
  let writeQueue: Promise<void> = Promise.resolve();

  function logFailure(err: unknown) {
    console.error('[draftCache] Failed to persist draft:', err);
  }

  function enqueue(write: () => Promise<void>): Promise<void> {
    // The queue's promises never reject: several call sites fire-and-forget
    // (`void` on onDestroy flushes and clearNow), so a rejected write would
    // surface as an unhandled rejection at teardown and be invisible
    // mid-session. Log it here instead and keep the chain alive.
    writeQueue = writeQueue.then(write).catch(logFailure);
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
    scheduleSave(key: string, value: string | DraftContent) {
      settleBeforeWrite(key);
      pendingKey = key;
      pendingValue = value;
      timer = setTimeout(() => {
        timer = null;
        pendingKey = null;
        void enqueue(() => saveDraft(key, value));
      }, delayMs);
    },
    saveNow(key: string, value: string | DraftContent): Promise<void> {
      settleBeforeWrite(key);
      return enqueue(() => saveDraft(key, value));
    },
    clearNow(key: string): Promise<void> {
      settleBeforeWrite(key);
      return enqueue(() => clearDraft(key));
    },
    /**
     * Run `task` in this writer's queue: after every write scheduled before
     * it (a pending debounced one is flushed first) and before any after. A
     * read-modify-write of a draft goes here, so no write can land between
     * its read and its write, and a `flush()` that starts later waits for it.
     *
     * Unlike the writes above, the returned promise rejects when `task`
     * does, so a caller can tell the user their change wasn't saved; the
     * queue logs the failure and carries on either way.
     */
    run(task: () => Promise<void>): Promise<void> {
      void flushPending();
      const result = writeQueue.then(task);
      writeQueue = result.catch(logFailure);
      return result;
    },
    flush: flushPending,
  };
}
