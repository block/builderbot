import { get, set, del, keys, entries, clear, createStore } from 'idb-keyval';
import { invokeCommand, isTauri } from './transport';

const CACHE_SCHEMA_VERSION = 1;
const MAX_CACHE_ENTRIES = 200;

/**
 * Tracks the last invalidation time per cache key. When a read starts, it
 * captures the current epoch for that key. If the key is invalidated while
 * the network request is in flight, the epoch advances and the stale write
 * is skipped — preventing pre-mutation data from repopulating the cache.
 */
const invalidationEpochs = new Map<string, number>();

function getEpoch(key: string): number {
  return invalidationEpochs.get(key) ?? 0;
}

function bumpEpoch(key: string): void {
  invalidationEpochs.set(key, Date.now());
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
  const epochAtStart = getEpoch(key);

  try {
    const data = await invokeCommand<T>(command, args);
    const fetchedAt = Date.now();
    // Skip the cache write if the key was invalidated while we were fetching —
    // writing would repopulate the cache with pre-mutation data.
    if (getEpoch(key) === epochAtStart) {
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

  const entry = await get<CacheEntry<T>>(key, store).catch(() => undefined);
  const isUsable = entry != null && entry.schemaVersion === CACHE_SCHEMA_VERSION;
  const isFresh = isUsable && !entry.stale && Date.now() - entry.fetchedAt < config.ttl;

  if (isUsable && isFresh) {
    return { data: entry.data, revalidating: null };
  }

  addInFlight(key);
  const epochAtStart = getEpoch(key);
  const network = invokeCommand<T>(command, args)
    .then(async (data) => {
      // Skip the cache write if the key was invalidated while we were fetching —
      // writing would repopulate the cache with pre-mutation data.
      if (getEpoch(key) === epochAtStart) {
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

/**
 * Evict the oldest cache entries (by fetchedAt) until the store is under the
 * MAX_CACHE_ENTRIES limit. Called after writes and on quota errors.
 */
async function evictIfNeeded(): Promise<void> {
  try {
    const store = getStore();
    const allEntries = await entries<string, CacheEntry<unknown>>(store);
    if (allEntries.length <= MAX_CACHE_ENTRIES) return;

    // Sort by fetchedAt ascending (oldest first) and evict the excess
    const sorted = allEntries.sort((a, b) => a[1].fetchedAt - b[1].fetchedAt);
    const toEvict = sorted.slice(0, sorted.length - MAX_CACHE_ENTRIES);
    await Promise.all(toEvict.map(([k]) => del(k, store)));
  } catch {
    // Best-effort eviction — don't let this block the caller
  }
}

/**
 * Write a cache entry, with quota-error recovery via LRU eviction.
 */
async function cacheSet<T>(key: string, entry: CacheEntry<T>): Promise<void> {
  const store = getStore();
  try {
    await set(key, entry, store);
  } catch (err) {
    // On quota error, evict old entries and retry once
    if (err instanceof DOMException && err.name === 'QuotaExceededError') {
      await evictIfNeeded();
      await set(key, entry, store).catch(() => {});
      return;
    }
    // Swallow other write errors — cache is best-effort
  }
  // Proactive eviction after successful writes
  evictIfNeeded();
}

/** Invalidate a specific cache entry. */
export async function invalidateCache(
  command: string,
  args?: Record<string, unknown>
): Promise<void> {
  if (isTauri) return;
  const key = cacheKey(command, args);
  bumpEpoch(key);
  await del(key, getStore()).catch(() => {});
}

/** Invalidate all entries for a command (regardless of args). */
export async function invalidateCacheByCommand(command: string): Promise<void> {
  if (isTauri) return;
  const store = getStore();
  const prefix = `${command}:`;
  const idbMatching = (await keys<string>(store)).filter((k) => k.startsWith(prefix));
  const inFlightMatching = [...inFlightKeys.keys()].filter((k) => k.startsWith(prefix));
  const toBump = new Set<string>([...idbMatching, ...inFlightMatching]);
  toBump.forEach(bumpEpoch);
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
  const store = getStore();
  const matches = (key: string): boolean => {
    const args = parseCacheArgs(key, command);
    if (!args) return false;
    return Object.entries(partialArgs).every(([argKey, argValue]) => args[argKey] === argValue);
  };
  const idbMatching = (await keys<string>(store)).filter(matches);
  const inFlightMatching = [...inFlightKeys.keys()].filter(matches);
  const toBump = new Set<string>([...idbMatching, ...inFlightMatching]);
  toBump.forEach(bumpEpoch);
  await Promise.all(idbMatching.map((k) => del(k, store)));
}

/** Mark all entries as stale so SWR serves them while revalidating. */
export async function markAllStale(): Promise<void> {
  if (isTauri) return;
  const store = getStore();
  const allEntries = await entries<string, CacheEntry<unknown>>(store);
  await Promise.all(allEntries.map(([k, entry]) => set(k, { ...entry, stale: true }, store)));
}

/** Remove all cached entries. */
export async function clearAllCache(): Promise<void> {
  if (isTauri) return;
  await clear(getStore());
}

// Exported for testing
export {
  cacheKey as _cacheKey,
  CACHE_SCHEMA_VERSION as _CACHE_SCHEMA_VERSION,
  MAX_CACHE_ENTRIES as _MAX_CACHE_ENTRIES,
  evictIfNeeded as _evictIfNeeded,
};
