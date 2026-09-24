import { get, set, del, keys, entries, clear, createStore, promisifyRequest } from 'idb-keyval';
import { invokeCommand, isTauri } from './transport';

/**
 * Stamped on every persisted cache entry and checked on read. Bump it whenever
 * the shape of any cached command response changes — otherwise the first load
 * after a deploy serves entries written by the previous build, and a field the
 * new UI depends on reads as `undefined` (e.g. `CommitTimelineItem.pipelineKind`,
 * whose absence silently re-enables the Rebase button mid-rebase).
 *
 * Entries that fail the check read as misses and are cleaned by the once-per-
 * session cache sweep. The cost of a bump is one cold-cache boot per client.
 *
 * The timeline boot snapshot in `commands.ts` is versioned by this same
 * constant, so one bump covers both layers that survive a deploy.
 */
export const CACHE_SCHEMA_VERSION = 2;
const CACHE_SWEEP_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * Every invalidation call takes the next sequence number synchronously, and
 * a fetch captures the current number when it starts. When the invalidation
 * later resolves which keys it covers (the command-wide and by-args variants
 * have to scan IDB first), it stamps them with its sequence; a fetch whose
 * write would land is skipped when its key carries a newer stamp.
 *
 * Stamping at call time rather than at scan time is what lets the change-feed
 * handlers run in one dispatch: the cache listener starts the invalidation,
 * the view store issues its forced reload right behind it, and that reload —
 * post-mutation by construction — is not mistaken for the pre-mutation fetch
 * the invalidation exists to block.
 */
let invalidationSequence = 0;
const keyInvalidatedAt = new Map<string, number>();

/**
 * Generation for full-cache invalidations. Unlike the per-key map, this also
 * covers keys that exist only as in-flight requests when the generation moves.
 */
let fullInvalidationEpoch = 0;

interface InvalidationEpoch {
  sequence: number;
  full: number;
}

function nextInvalidationSequence(): number {
  return ++invalidationSequence;
}

function markInvalidated(key: string, sequence: number): void {
  // Max, not overwrite: a slow scan settling after a later invalidation must
  // not roll the key's stamp back and let a fetch the later one blocked through.
  keyInvalidatedAt.set(key, Math.max(keyInvalidatedAt.get(key) ?? 0, sequence));
}

function captureEpoch(): InvalidationEpoch {
  return { sequence: invalidationSequence, full: fullInvalidationEpoch };
}

function isEpochCurrent(key: string, epoch: InvalidationEpoch): boolean {
  return (keyInvalidatedAt.get(key) ?? 0) <= epoch.sequence && fullInvalidationEpoch === epoch.full;
}

/**
 * Refcount of cache keys with an active network fetch. Scoped invalidators
 * union this with the IDB key set so first-load races (key not yet written to
 * IDB) still get their epoch bumped — otherwise the in-flight fetch would
 * resolve and write pre-mutation data into the invalidated namespace.
 */
const inFlightKeys = new Map<string, number>();

function addInFlight(key: string): void {
  inFlightKeys.set(key, (inFlightKeys.get(key) ?? 0) + 1);
}

function removeInFlight(key: string): void {
  const n = inFlightKeys.get(key) ?? 0;
  if (n <= 1) inFlightKeys.delete(key);
  else inFlightKeys.set(key, n - 1);
}

let cacheStore: ReturnType<typeof createStore> | undefined;

function getStore() {
  if (!cacheStore) {
    cacheStore = createStore('staged-cache', 'responses');
  }
  return cacheStore;
}

interface CacheEntry<T> {
  key: string;
  data: T;
  fetchedAt: number;
  schemaVersion: number;
  stale?: boolean;
}

export interface CacheConfig {
  ttl: number;
  /**
   * Skip the IDB read and always go to the network. The fresh response is
   * still written back to IDB (subject to the same epoch race protection).
   * Use this when a caller explicitly wants a real revalidation and not a
   * potentially within-TTL cached value — e.g. `getBranchTimeline({ force })`.
   */
  bypassRead?: boolean;
}

/**
 * Result of a cached command call.
 *
 * `data` is the best value available immediately (cached if usable, otherwise
 * from the network). When `revalidating` is non-null, a network fetch is in
 * flight: await it to get the fresh value. Callers can render `data` instantly
 * and then re-render once `revalidating` resolves.
 */
export interface SwrResult<T> {
  data: T;
  revalidating: Promise<T> | null;
}

function cacheKey(command: string, args?: Record<string, unknown>): string {
  const argsStr = args ? JSON.stringify(args, Object.keys(args).sort()) : '';
  return `${command}:${argsStr}`;
}

/**
 * Stale-while-revalidate wrapper around invokeCommand.
 *
 * Yields:
 *   1. Cached data (if available and schema matches) — instant
 *   2. Fresh network data — if cached data is stale or expired
 *
 * Fresh entries (within TTL and not marked stale) short-circuit with
 * only the cache yield. If no cache exists, only the network result
 * is yielded.
 */
export async function* cachedInvoke<T>(
  command: string,
  args: Record<string, unknown> | undefined,
  config: CacheConfig
): AsyncGenerator<{ data: T; source: 'cache' | 'network'; fetchedAt: number }> {
  if (isTauri) {
    const data = await invokeCommand<T>(command, args);
    yield { data, source: 'network', fetchedAt: Date.now() };
    return;
  }

  const key = cacheKey(command, args);
  const store = getStore();
  // Stamped at request time, before the IDB read: an invalidation called from
  // here on postdates this request and must not be mistaken for one this
  // request's response could predate.
  const epochAtStart = captureEpoch();

  let entry: CacheEntry<T> | undefined;
  let isUsable = false;
  if (!config.bypassRead) {
    entry = await get<CacheEntry<T>>(key, store).catch(() => undefined);
    isUsable = entry != null && entry.schemaVersion === CACHE_SCHEMA_VERSION;
    const isFresh = isUsable && !entry!.stale && Date.now() - entry!.fetchedAt < config.ttl;

    if (isUsable) {
      yield { data: entry!.data, source: 'cache', fetchedAt: entry!.fetchedAt };
    }

    if (isFresh) return;
  }

  addInFlight(key);

  try {
    const data = await invokeCommand<T>(command, args);
    const fetchedAt = Date.now();
    // Skip the cache write if the key was invalidated while we were fetching —
    // writing would repopulate the cache with pre-mutation data.
    if (isEpochCurrent(key, epochAtStart)) {
      await cacheSet(key, {
        key,
        data,
        fetchedAt,
        schemaVersion: CACHE_SCHEMA_VERSION,
      } satisfies CacheEntry<T>);
    }
    yield { data, source: 'network', fetchedAt };
  } catch (err) {
    // With bypassRead the caller asked for a real fetch — surface the failure
    // instead of silently falling back to whatever IDB had.
    if (config.bypassRead || !isUsable) throw err;
    console.warn(`[cache] Network error for ${command}, serving stale cache`, err);
  } finally {
    removeInFlight(key);
  }
}

/**
 * Like invokeCommand, but with SWR caching.
 *
 * Returns `{ data, revalidating }`:
 * - `data` is the best value available immediately (cached if usable, else network).
 * - `revalidating` is non-null when a background network fetch is in flight;
 *   callers can await it to get the fresh value.
 */
export async function cachedCommand<T>(
  command: string,
  args: Record<string, unknown> | undefined,
  config: CacheConfig
): Promise<SwrResult<T>> {
  if (isTauri) {
    const data = await invokeCommand<T>(command, args);
    return { data, revalidating: null };
  }

  const key = cacheKey(command, args);
  const store = getStore();
  // Stamped at request time, before any IDB read (see cachedInvoke).
  const epochAtStart = captureEpoch();

  if (config.bypassRead) {
    addInFlight(key);
    try {
      const data = await invokeCommand<T>(command, args);
      // Skip the cache write if the key was invalidated while we were fetching —
      // writing would repopulate the cache with pre-mutation data.
      if (isEpochCurrent(key, epochAtStart)) {
        await cacheSet(key, {
          key,
          data,
          fetchedAt: Date.now(),
          schemaVersion: CACHE_SCHEMA_VERSION,
        } satisfies CacheEntry<T>);
      }
      return { data, revalidating: null };
    } finally {
      removeInFlight(key);
    }
  }

  const entry = await get<CacheEntry<T>>(key, store).catch(() => undefined);
  const isUsable = entry != null && entry.schemaVersion === CACHE_SCHEMA_VERSION;
  const isFresh = isUsable && !entry.stale && Date.now() - entry.fetchedAt < config.ttl;

  if (isUsable && isFresh) {
    return { data: entry.data, revalidating: null };
  }

  addInFlight(key);
  const network = invokeCommand<T>(command, args)
    .then(async (data) => {
      // Skip the cache write if the key was invalidated while we were fetching —
      // writing would repopulate the cache with pre-mutation data.
      if (isEpochCurrent(key, epochAtStart)) {
        await cacheSet(key, {
          key,
          data,
          fetchedAt: Date.now(),
          schemaVersion: CACHE_SCHEMA_VERSION,
        } satisfies CacheEntry<T>);
      }
      return data;
    })
    .finally(() => removeInFlight(key));

  if (isUsable) {
    // Stale/expired but usable — return cached data immediately and let the
    // caller await revalidation. Swallow network errors so the stale entry
    // remains the resolved value (mirrors cachedInvoke behavior).
    const revalidating = network.catch((err) => {
      console.warn(`[cache] Network error for ${command}, serving stale cache`, err);
      return entry.data;
    });
    return { data: entry.data, revalidating };
  }

  // Miss — must await the network before we can return anything usable.
  const data = await network;
  return { data, revalidating: null };
}

async function evictOldestHalf(): Promise<void> {
  const store = getStore();
  const allEntries = await entries<string, CacheEntry<unknown>>(store);
  if (allEntries.length === 0) return;

  const sorted = allEntries.sort((a, b) => a[1].fetchedAt - b[1].fetchedAt);
  const toEvict = sorted.slice(0, Math.ceil(sorted.length / 2));
  await Promise.all(toEvict.map(([k]) => del(k, store)));
}

function isQuotaExceededError(err: unknown): boolean {
  return err instanceof DOMException && err.name === 'QuotaExceededError';
}

/**
 * Write a cache entry, with quota-error recovery by evicting the oldest half
 * and retrying once.
 */
async function cacheSet<T>(key: string, entry: CacheEntry<T>): Promise<void> {
  const store = getStore();
  try {
    await set(key, entry, store);
  } catch (err) {
    if (isQuotaExceededError(err)) {
      try {
        await evictOldestHalf();
        await set(key, entry, store);
      } catch {
        // Cache is best-effort.
      }
    }
    // Swallow write errors — cache is best-effort.
  }
}

function isSweepable(entry: Partial<CacheEntry<unknown>> | undefined, now: number): boolean {
  return (
    entry == null ||
    entry.schemaVersion !== CACHE_SCHEMA_VERSION ||
    typeof entry.fetchedAt !== 'number' ||
    now - entry.fetchedAt > CACHE_SWEEP_MAX_AGE_MS
  );
}

/**
 * Remove stale schema and week-old entries. Intended to run once per app
 * session — during boot, while hydration is still writing. Check and delete
 * happen on the live record inside one readwrite cursor so a write that lands
 * mid-sweep (its transaction serializes before or after this one, never
 * between a read and its delete) is seen with its fresh fetchedAt and kept.
 */
export async function sweepCache(): Promise<void> {
  if (isTauri) return;
  try {
    const store = getStore();
    const now = Date.now();
    await store('readwrite', (objectStore) => {
      objectStore.openCursor().onsuccess = function () {
        const cursor = this.result;
        if (!cursor) return;
        if (isSweepable(cursor.value as Partial<CacheEntry<unknown>> | undefined, now)) {
          cursor.delete();
        }
        cursor.continue();
      };
      return promisifyRequest(objectStore.transaction);
    });
  } catch {
    // Best-effort sweep — don't let cache maintenance block boot.
  }
}

/** Invalidate a specific cache entry. */
export async function invalidateCache(
  command: string,
  args?: Record<string, unknown>
): Promise<void> {
  if (isTauri) return;
  const key = cacheKey(command, args);
  markInvalidated(key, nextInvalidationSequence());
  await del(key, getStore()).catch(() => {});
}

/**
 * Invalidate all entries for a command (regardless of args).
 *
 * The IDB delete is best-effort ordering-wise: a post-call fetch that comes
 * back before the key scan settles has its fresh write deleted here and is
 * refetched on the next read — a miss, never stale data.
 */
export async function invalidateCacheByCommand(command: string): Promise<void> {
  if (isTauri) return;
  const sequence = nextInvalidationSequence();
  const store = getStore();
  const prefix = `${command}:`;
  const idbMatching = (await keys<string>(store)).filter((k) => k.startsWith(prefix));
  const inFlightMatching = [...inFlightKeys.keys()].filter((k) => k.startsWith(prefix));
  const toMark = new Set<string>([...idbMatching, ...inFlightMatching]);
  toMark.forEach((k) => markInvalidated(k, sequence));
  await Promise.all(idbMatching.map((k) => del(k, store)));
}

function parseCacheArgs(key: string, command: string): Record<string, unknown> | undefined {
  const prefix = `${command}:`;
  if (!key.startsWith(prefix)) return undefined;

  try {
    const parsed = JSON.parse(key.slice(prefix.length)) as unknown;
    if (parsed == null || typeof parsed !== 'object' || Array.isArray(parsed)) return undefined;
    return parsed as Record<string, unknown>;
  } catch {
    return undefined;
  }
}

/** Invalidate entries for a command whose cached args include all partial args. */
export async function invalidateCacheByArgs(
  command: string,
  partialArgs: Record<string, unknown>
): Promise<void> {
  if (isTauri) return;
  const sequence = nextInvalidationSequence();
  const store = getStore();
  const matches = (key: string): boolean => {
    const args = parseCacheArgs(key, command);
    if (!args) return false;
    return Object.entries(partialArgs).every(([argKey, argValue]) => args[argKey] === argValue);
  };
  const idbMatching = (await keys<string>(store)).filter(matches);
  const inFlightMatching = [...inFlightKeys.keys()].filter(matches);
  const toMark = new Set<string>([...idbMatching, ...inFlightMatching]);
  toMark.forEach((k) => markInvalidated(k, sequence));
  await Promise.all(idbMatching.map((k) => del(k, store)));
}

/** Mark all entries as stale so SWR serves them while revalidating. */
export async function markAllStale(): Promise<void> {
  if (isTauri) return;
  // Move the generation before the first IndexedDB await so every request
  // already in flight is barred from writing a pre-invalidation response,
  // including first-load keys that do not exist in the store yet.
  fullInvalidationEpoch += 1;
  const store = getStore();
  const allEntries = await entries<string, CacheEntry<unknown>>(store);
  await Promise.all(allEntries.map(([k, entry]) => set(k, { ...entry, stale: true }, store)));
}

/** Remove all cached entries. */
export async function clearAllCache(): Promise<void> {
  if (isTauri) return;
  fullInvalidationEpoch += 1;
  await clear(getStore());
}

// Exported for testing
export { cacheKey as _cacheKey };
