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
 * constant, so one bump covers both layers that survive a deploy. It also
 * guards the desktop app's persisted branch timelines (the one command that
 * uses this layer under Tauri), so a `BranchTimeline` shape change needs a
 * bump even when nothing else cached has changed.
 */
export const CACHE_SCHEMA_VERSION = 3;
const CACHE_SWEEP_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * Commands whose responses are persisted in IndexedDB under Tauri as well as
 * on the web. The desktop app otherwise skips this layer entirely: the native
 * backend answers most commands in a few milliseconds, and a stale read would
 * cost more than it saves. A branch timeline is the exception — building one
 * is a chain of git subprocesses that takes one to four seconds after launch,
 * so its last result is kept so a card can paint it while the rebuild runs.
 */
const TAURI_PERSISTED_COMMANDS: ReadonlySet<string> = new Set(['get_branch_timeline']);

/** Whether `command` goes through the IndexedDB layer on this platform. */
function isPersisted(command: string): boolean {
  return !isTauri || TAURI_PERSISTED_COMMANDS.has(command);
}

/**
 * Every invalidation call takes the next sequence number synchronously and
 * stamps the keys it covers with it. A fetch captures the current number as
 * its network call goes out, and skips its cache write when its key carries a
 * newer stamp.
 *
 * Stamping at call time rather than after an IDB scan is what lets the
 * change-feed handlers run in one dispatch: the cache listener starts the
 * invalidation, the view store issues its forced reload right behind it, and
 * that reload — post-mutation by construction — carries the new sequence and
 * is not mistaken for the pre-mutation fetch the invalidation exists to block.
 * It is also what covers a key that so far exists only as an in-flight
 * request: the command-wide and by-args variants stamp the matching in-flight
 * keys before they scan IDB, so a response that lands mid-scan is already
 * blocked rather than written where no delete will ever reach it.
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
 * stamp the matching keys here synchronously at call time, before their IDB
 * scan, so first-load races (key not yet written to IDB) are blocked even
 * when the response lands mid-scan — otherwise the in-flight fetch would
 * write pre-mutation data into the invalidated namespace and nothing would
 * delete it.
 *
 * Every fetch registers here in the same synchronous step that captures its
 * epoch, so a fetch holding an older sequence than an invalidation is always
 * visible to that invalidation's in-flight pass.
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
  /** See `CacheConfig.scope`. */
  scope?: string;
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
  /**
   * Keep at most this many persisted entries for the command. After a write
   * pushes the command over the cap, the entries with the oldest `fetchedAt`
   * are evicted. Entries are rewritten on every fetch, so "oldest fetched" is
   * "least recently viewed" for anything the UI revalidates when shown.
   */
  maxEntries?: number;
  /**
   * Opaque identity of the data the entry describes, stored alongside it and
   * handed back by `readCachedEntries`. The cache does not interpret it; a
   * caller that seeds from persisted entries compares it to the live identity
   * (e.g. a branch's project, worktree and workspace) and drops entries whose
   * subject has since moved or been deleted.
   */
  scope?: string;
}

/** A usable persisted entry, as returned by `readCachedEntries`. */
export interface PersistedEntry<T> {
  args: Record<string, unknown> | undefined;
  data: T;
  fetchedAt: number;
  scope: string | undefined;
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
  if (!isPersisted(command)) {
    const data = await invokeCommand<T>(command, args);
    yield { data, source: 'network', fetchedAt: Date.now() };
    return;
  }

  const key = cacheKey(command, args);

  let entry: CacheEntry<T> | undefined;
  let isUsable = false;
  if (!config.bypassRead) {
    entry = await cacheGet<T>(key);
    isUsable = entry != null && entry.schemaVersion === CACHE_SCHEMA_VERSION;
    const isFresh = isUsable && !entry!.stale && Date.now() - entry!.fetchedAt < config.ttl;

    if (isUsable) {
      yield { data: entry!.data, source: 'cache', fetchedAt: entry!.fetchedAt };
    }

    if (isFresh) return;
  }

  // Registered and stamped as the network call goes out, after the IDB read:
  // an invalidation that landed during the read predates this network call,
  // so its response is post-mutation and may be cached. The two calls stay
  // adjacent so an invalidation can never see one without the other.
  addInFlight(key);
  const epochAtStart = captureEpoch();

  try {
    const data = await invokeCommand<T>(command, args);
    const fetchedAt = Date.now();
    // Skip the cache write if the key was invalidated while we were fetching —
    // writing would repopulate the cache with pre-mutation data.
    if (isEpochCurrent(key, epochAtStart)) {
      await cacheSet(
        key,
        {
          key,
          data,
          fetchedAt,
          schemaVersion: CACHE_SCHEMA_VERSION,
          scope: config.scope,
        } satisfies CacheEntry<T>,
        config
      );
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
  if (!isPersisted(command)) {
    const data = await invokeCommand<T>(command, args);
    return { data, revalidating: null };
  }

  const key = cacheKey(command, args);

  if (config.bypassRead) {
    addInFlight(key);
    const epochAtStart = captureEpoch();
    try {
      const data = await invokeCommand<T>(command, args);
      // Skip the cache write if the key was invalidated while we were fetching —
      // writing would repopulate the cache with pre-mutation data.
      if (isEpochCurrent(key, epochAtStart)) {
        await cacheSet(
          key,
          {
            key,
            data,
            fetchedAt: Date.now(),
            schemaVersion: CACHE_SCHEMA_VERSION,
            scope: config.scope,
          } satisfies CacheEntry<T>,
          config
        );
      }
      return { data, revalidating: null };
    } finally {
      removeInFlight(key);
    }
  }

  const entry = await cacheGet<T>(key);
  const isUsable = entry != null && entry.schemaVersion === CACHE_SCHEMA_VERSION;
  const isFresh = isUsable && !entry.stale && Date.now() - entry.fetchedAt < config.ttl;

  if (isUsable && isFresh) {
    return { data: entry.data, revalidating: null };
  }

  // Registered and stamped as the network call goes out, after the IDB read
  // (see cachedInvoke).
  addInFlight(key);
  const epochAtStart = captureEpoch();
  const network = invokeCommand<T>(command, args)
    .then(async (data) => {
      // Skip the cache write if the key was invalidated while we were fetching —
      // writing would repopulate the cache with pre-mutation data.
      if (isEpochCurrent(key, epochAtStart)) {
        await cacheSet(
          key,
          {
            key,
            data,
            fetchedAt: Date.now(),
            schemaVersion: CACHE_SCHEMA_VERSION,
            scope: config.scope,
          } satisfies CacheEntry<T>,
          config
        );
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
 * Read a cache entry. Any IndexedDB failure — including a store that cannot
 * be opened at all — reads as a miss so the caller goes to the network.
 */
async function cacheGet<T>(key: string): Promise<CacheEntry<T> | undefined> {
  try {
    return await get<CacheEntry<T>>(key, getStore());
  } catch {
    return undefined;
  }
}

/**
 * Write a cache entry, with quota-error recovery by evicting the oldest half
 * and retrying once. A successful write then trims the command to
 * `config.maxEntries` in the background.
 */
async function cacheSet<T>(key: string, entry: CacheEntry<T>, config?: CacheConfig): Promise<void> {
  try {
    const store = getStore();
    try {
      await set(key, entry, store);
    } catch (err) {
      if (!isQuotaExceededError(err)) return;
      await evictOldestHalf();
      await set(key, entry, store);
    }
  } catch {
    // Swallow write errors — cache is best-effort.
    return;
  }
  if (config?.maxEntries != null) {
    void enforceEntryCap(commandOf(key), config.maxEntries);
  }
}

function commandOf(key: string): string {
  return key.slice(0, key.indexOf(':'));
}

/** Key range covering every entry written for `command`. */
function commandKeyRange(command: string): IDBKeyRange {
  const prefix = `${command}:`;
  return IDBKeyRange.bound(prefix, `${prefix}\uffff`, false, true);
}

/** One in-flight trim per command: concurrent writes at project open share it. */
const entryCapEnforcement = new Map<string, Promise<void>>();

/**
 * Evict the oldest-fetched entries of `command` beyond `max`. The key count
 * is one cheap getAllKeys; the values are only read when the command is over
 * the cap, which happens once per newly seen key, not on every refetch.
 */
function enforceEntryCap(command: string, max: number): Promise<void> {
  const inFlight = entryCapEnforcement.get(command);
  if (inFlight) return inFlight;
  const run = (async () => {
    try {
      const store = getStore();
      const range = commandKeyRange(command);
      const count = (await store('readonly', (os) => promisifyRequest(os.getAllKeys(range))))
        .length;
      if (count <= max) return;
      const rows = (await store('readonly', (os) => promisifyRequest(os.getAll(range)))) as Array<
        Partial<CacheEntry<unknown>>
      >;
      const overflow = rows
        .filter((row) => typeof row?.key === 'string')
        .sort((a, b) => (a.fetchedAt ?? 0) - (b.fetchedAt ?? 0))
        .slice(0, Math.max(0, rows.length - max));
      await Promise.all(overflow.map((row) => del(row.key!, store)));
    } catch {
      // Best-effort bound; the next write tries again.
    } finally {
      entryCapEnforcement.delete(command);
    }
  })();
  entryCapEnforcement.set(command, run);
  return run;
}

/**
 * Every usable persisted entry for `command`, oldest fetch first. For seeding
 * an in-memory cache at boot so the first paint does not wait on a per-key
 * IndexedDB read. Entries from another schema version are skipped (the sweep
 * removes them); any IndexedDB failure yields an empty list.
 */
export async function readCachedEntries<T>(command: string): Promise<PersistedEntry<T>[]> {
  if (!isPersisted(command)) return [];
  try {
    const store = getStore();
    const rows = (await store('readonly', (os) =>
      promisifyRequest(os.getAll(commandKeyRange(command)))
    )) as Array<Partial<CacheEntry<T>> | undefined>;
    const usable: PersistedEntry<T>[] = [];
    for (const row of rows) {
      if (
        row == null ||
        typeof row.key !== 'string' ||
        row.schemaVersion !== CACHE_SCHEMA_VERSION ||
        typeof row.fetchedAt !== 'number' ||
        row.data === undefined
      ) {
        continue;
      }
      usable.push({
        args: parseCacheArgs(row.key, command),
        data: row.data,
        fetchedAt: row.fetchedAt,
        scope: typeof row.scope === 'string' ? row.scope : undefined,
      });
    }
    return usable.sort((a, b) => a.fetchedAt - b.fetchedAt);
  } catch {
    return [];
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
 * happen on the live records inside one readwrite transaction so a write that
 * lands mid-sweep (its transaction serializes before or after this one, never
 * between the read and its delete) is seen with its fresh fetchedAt and kept.
 *
 * The read is one getAllKeys plus one getAll rather than a cursor: a
 * readwrite transaction holds the store's exclusive lock for as long as it
 * has requests outstanding, and a cursor costs one event-loop hop per record
 * while boot-time cachedInvoke reads queue behind it. Two hops plus the
 * deletes keeps that stall bounded however large the store has grown.
 */
export async function sweepCache(): Promise<void> {
  try {
    const store = getStore();
    const now = Date.now();
    await store('readwrite', (objectStore) => {
      // Requests on one transaction complete in issue order, so the keys are
      // in hand when the values arrive, and both are in key order with no
      // write between them, so the two arrays line up index for index.
      const keysRequest = objectStore.getAllKeys();
      const valuesRequest = objectStore.getAll();
      valuesRequest.onsuccess = () => {
        const allKeys = keysRequest.result;
        const values = valuesRequest.result as Array<Partial<CacheEntry<unknown>> | undefined>;
        for (let i = 0; i < allKeys.length; i++) {
          if (isSweepable(values[i], now)) objectStore.delete(allKeys[i]);
        }
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
  if (!isPersisted(command)) return;
  const key = cacheKey(command, args);
  markInvalidated(key, nextInvalidationSequence());
  try {
    await del(key, getStore());
  } catch {
    // Best-effort: the stamp above already blocks an in-flight write.
  }
}

/**
 * Shared body of the scoped invalidators.
 *
 * The in-flight keys are stamped synchronously, before the IDB key scan: a
 * fetch that was in flight at call time may land during the scan, and if its
 * key is not in IDB yet the scan has nothing to delete, so an unstamped write
 * would be served as fresh for the whole TTL. Every fetch that starts after
 * this point captures a sequence >= ours and passes the check by
 * construction, so the post-scan pass only has to delete.
 *
 * That delete is best-effort ordering-wise: a post-call fetch that comes back
 * before the key scan settles has its fresh write deleted here and is
 * refetched on the next read — a miss, never stale data.
 */
async function invalidateMatching(matches: (key: string) => boolean): Promise<void> {
  const sequence = nextInvalidationSequence();
  for (const key of inFlightKeys.keys()) {
    if (matches(key)) markInvalidated(key, sequence);
  }
  try {
    const store = getStore();
    const idbMatching = (await keys<string>(store)).filter(matches);
    await Promise.all(idbMatching.map((k) => del(k, store)));
  } catch {
    // Best-effort: the stamps above already block in-flight writes.
  }
}

/** Invalidate all entries for a command (regardless of args). */
export async function invalidateCacheByCommand(command: string): Promise<void> {
  if (!isPersisted(command)) return;
  const prefix = `${command}:`;
  await invalidateMatching((key) => key.startsWith(prefix));
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
  if (!isPersisted(command)) return;
  await invalidateMatching((key) => {
    const args = parseCacheArgs(key, command);
    if (!args) return false;
    return Object.entries(partialArgs).every(([argKey, argValue]) => args[argKey] === argValue);
  });
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
  fullInvalidationEpoch += 1;
  try {
    await clear(getStore());
  } catch {
    // Best-effort: the epoch bump already blocks in-flight writes.
  }
}

// Exported for testing
export { cacheKey as _cacheKey };
