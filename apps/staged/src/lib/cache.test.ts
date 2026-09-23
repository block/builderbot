import 'fake-indexeddb/auto';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createStore, entries, set } from 'idb-keyval';

// Mock transport — web mode (isTauri = false) with controllable invokeCommand
const mockInvoke = vi.fn();
vi.mock('./transport', () => ({
  isTauri: false,
  invokeCommand: (...args: unknown[]) => mockInvoke(...args),
}));

import {
  cachedInvoke,
  cachedCommand,
  invalidateCache,
  invalidateCacheByArgs,
  invalidateCacheByCommand,
  markAllStale,
  clearAllCache,
  sweepCache,
  CACHE_SCHEMA_VERSION,
  _cacheKey,
} from './cache';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(async () => {
  mockInvoke.mockReset();
  await clearAllCache();
});

describe('cacheKey', () => {
  it('produces deterministic keys regardless of arg order', () => {
    expect(_cacheKey('cmd', { b: 2, a: 1 })).toBe(_cacheKey('cmd', { a: 1, b: 2 }));
  });

  it('produces different keys for different commands', () => {
    expect(_cacheKey('foo', { a: 1 })).not.toBe(_cacheKey('bar', { a: 1 }));
  });

  it('handles undefined args', () => {
    expect(_cacheKey('cmd')).toBe('cmd:');
  });
});

describe('schema version', () => {
  /** Write an entry the way a build one schema version behind would have. */
  async function writePreviousVersionEntry(command: string, data: unknown): Promise<void> {
    const key = _cacheKey(command);
    await set(
      key,
      { key, data, fetchedAt: Date.now(), schemaVersion: CACHE_SCHEMA_VERSION - 1 },
      createStore('staged-cache', 'responses')
    );
  }

  it('never yields an entry written under a previous schema version', async () => {
    await writePreviousVersionEntry('cmd', 'old-shape');
    mockInvoke.mockResolvedValue('new-shape');

    const results = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      results.push(r);
    }

    // A within-TTL entry would normally short-circuit the fetch entirely; the
    // version mismatch has to demote it to a miss, or the deploy that changed
    // the payload serves the old shape with no network correction.
    expect(results).toEqual([
      { data: 'new-shape', source: 'network', fetchedAt: expect.any(Number) },
    ]);
  });

  it('treats a previous-version entry as a miss in cachedCommand', async () => {
    await writePreviousVersionEntry('cmd', 'old-shape');
    mockInvoke.mockResolvedValue('new-shape');

    await expect(cachedCommand('cmd', undefined, { ttl: 60_000 })).resolves.toEqual({
      data: 'new-shape',
      revalidating: null,
    });
  });
});

describe('cachedInvoke', () => {
  it('yields only network result on cache miss', async () => {
    mockInvoke.mockResolvedValue({ items: [1, 2] });

    const results = [];
    for await (const r of cachedInvoke('list', undefined, { ttl: 60_000 })) {
      results.push(r);
    }

    expect(results).toEqual([
      { data: { items: [1, 2] }, source: 'network', fetchedAt: expect.any(Number) },
    ]);
    expect(mockInvoke).toHaveBeenCalledWith('list', undefined);
  });

  it('short-circuits with only cache when entry is fresh', async () => {
    mockInvoke.mockResolvedValue('first');

    // Prime the cache
    const primeResults = [];
    for await (const r of cachedInvoke('cmd', { id: '1' }, { ttl: 60_000 })) {
      primeResults.push(r);
    }
    expect(primeResults).toHaveLength(1);

    // Second call within TTL should yield only cache (no network call)
    mockInvoke.mockResolvedValue('second');
    const results = [];
    for await (const r of cachedInvoke('cmd', { id: '1' }, { ttl: 60_000 })) {
      results.push(r);
    }

    expect(results).toEqual([{ data: 'first', source: 'cache', fetchedAt: expect.any(Number) }]);
    expect(mockInvoke).toHaveBeenCalledTimes(1); // no second network call
  });

  it('yields expired cache then revalidates from network', async () => {
    mockInvoke.mockResolvedValue('data');

    // Prime with ttl=1ms
    for await (const _ of cachedInvoke('cmd', undefined, { ttl: 1 })) {
      /* consume */
    }

    // Wait for expiry
    await new Promise((r) => setTimeout(r, 5));

    mockInvoke.mockResolvedValue('fresh');
    const results = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 1 })) {
      results.push(r);
    }

    // Expired entry is still usable — yield stale cache, then network
    expect(results).toEqual([
      { data: 'data', source: 'cache', fetchedAt: expect.any(Number) },
      { data: 'fresh', source: 'network', fetchedAt: expect.any(Number) },
    ]);
  });

  it('swallows network errors when usable stale cache exists', async () => {
    mockInvoke.mockResolvedValue('cached-data');

    // Prime
    for await (const _ of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      /* consume */
    }

    // Mark stale so revalidation is attempted
    await markAllStale();

    // Network fails on second call
    mockInvoke.mockRejectedValue(new Error('offline'));
    const results = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      results.push(r);
    }

    // Stale cache is still served despite network failure
    expect(results).toEqual([
      { data: 'cached-data', source: 'cache', fetchedAt: expect.any(Number) },
    ]);
  });

  it('throws network errors when no valid cache exists', async () => {
    mockInvoke.mockRejectedValue(new Error('offline'));

    const results = [];
    let thrown: Error | undefined;
    try {
      for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
        results.push(r);
      }
    } catch (e) {
      thrown = e as Error;
    }

    expect(thrown?.message).toBe('offline');
    expect(results).toEqual([]);
  });
});

describe('cachedInvoke with bypassRead', () => {
  it('skips the cache yield and goes to network even when a fresh entry exists', async () => {
    mockInvoke.mockResolvedValue('cached');
    for await (const _ of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      /* prime */
    }

    mockInvoke.mockResolvedValue('fresh');
    const results = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000, bypassRead: true })) {
      results.push(r);
    }

    expect(results).toEqual([{ data: 'fresh', source: 'network', fetchedAt: expect.any(Number) }]);
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it('writes the fresh response back to IDB so subsequent reads are warm', async () => {
    mockInvoke.mockResolvedValue('cached');
    for await (const _ of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      /* prime */
    }

    mockInvoke.mockResolvedValue('fresh');
    for await (const _ of cachedInvoke('cmd', undefined, { ttl: 60_000, bypassRead: true })) {
      /* consume */
    }

    mockInvoke.mockResolvedValue('should-not-be-used');
    const results = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      results.push(r);
    }

    expect(results).toEqual([{ data: 'fresh', source: 'cache', fetchedAt: expect.any(Number) }]);
  });

  it('rethrows network errors instead of falling back to the cached entry', async () => {
    mockInvoke.mockResolvedValue('cached');
    for await (const _ of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      /* prime */
    }

    mockInvoke.mockRejectedValue(new Error('offline'));
    let thrown: Error | undefined;
    const results = [];
    try {
      for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000, bypassRead: true })) {
        results.push(r);
      }
    } catch (e) {
      thrown = e as Error;
    }

    expect(thrown?.message).toBe('offline');
    expect(results).toEqual([]);
  });

  it('honors epoch invalidation during an in-flight bypassRead fetch', async () => {
    mockInvoke.mockResolvedValue('cached');
    for await (const _ of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      /* prime */
    }

    let resolveNetwork!: (value: string) => void;
    const pending = new Promise<string>((res) => {
      resolveNetwork = res;
    });
    mockInvoke.mockReturnValueOnce(pending);

    const consumer = (async () => {
      const out = [];
      for await (const r of cachedInvoke<string>('cmd', undefined, {
        ttl: 60_000,
        bypassRead: true,
      })) {
        out.push(r);
      }
      return out;
    })();

    // Let cachedInvoke reach the network step
    await Promise.resolve();
    await Promise.resolve();

    await invalidateCache('cmd');

    resolveNetwork('post-invalidate');
    const results = await consumer;

    expect(results).toEqual([
      { data: 'post-invalidate', source: 'network', fetchedAt: expect.any(Number) },
    ]);

    // The epoch bump should have suppressed the write — a subsequent read is a miss.
    mockInvoke.mockResolvedValueOnce('fresh');
    const followUp = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      followUp.push(r);
    }
    expect(followUp).toEqual([{ data: 'fresh', source: 'network', fetchedAt: expect.any(Number) }]);
  });
});

describe('cachedCommand', () => {
  it('returns network result with no revalidation on cache miss', async () => {
    mockInvoke.mockResolvedValue('value');

    const result = await cachedCommand<string>('cmd', undefined, { ttl: 60_000 });

    expect(result.data).toBe('value');
    expect(result.revalidating).toBeNull();
  });

  it('returns only cached data when entry is fresh', async () => {
    mockInvoke.mockResolvedValue('v1');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    mockInvoke.mockResolvedValue('v2');
    const result = await cachedCommand<string>('cmd', undefined, { ttl: 60_000 });

    // Fresh entry short-circuits — no network call
    expect(result.data).toBe('v1');
    expect(result.revalidating).toBeNull();
    expect(mockInvoke).toHaveBeenCalledTimes(1);
  });

  it('returns stale data with a revalidating promise that resolves to fresh', async () => {
    mockInvoke.mockResolvedValue('cached');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    await markAllStale();

    mockInvoke.mockResolvedValue('fresh');
    const result = await cachedCommand<string>('cmd', undefined, { ttl: 60_000 });

    expect(result.data).toBe('cached');
    expect(result.revalidating).not.toBeNull();
    await expect(result.revalidating).resolves.toBe('fresh');
  });

  it('keeps revalidating promise resolving to cached data when network fails', async () => {
    mockInvoke.mockResolvedValue('cached-data');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    await markAllStale();

    mockInvoke.mockRejectedValue(new Error('offline'));
    const result = await cachedCommand<string>('cmd', undefined, { ttl: 60_000 });

    expect(result.data).toBe('cached-data');
    await expect(result.revalidating).resolves.toBe('cached-data');
  });

  it('throws on miss when the network fails', async () => {
    mockInvoke.mockRejectedValue(new Error('offline'));
    await expect(cachedCommand('cmd', undefined, { ttl: 60_000 })).rejects.toThrow('offline');
  });

  it('skips a fresh cache entry when bypassRead is set', async () => {
    mockInvoke.mockResolvedValue('cached');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    mockInvoke.mockResolvedValue('fresh');
    const result = await cachedCommand<string>('cmd', undefined, {
      ttl: 60_000,
      bypassRead: true,
    });

    expect(result).toEqual({ data: 'fresh', revalidating: null });
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it('writes a bypassRead response back to IDB so later reads are warm', async () => {
    mockInvoke.mockResolvedValue('cached');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    mockInvoke.mockResolvedValue('fresh');
    await cachedCommand('cmd', undefined, { ttl: 60_000, bypassRead: true });

    mockInvoke.mockResolvedValue('should-not-be-used');
    const result = await cachedCommand<string>('cmd', undefined, { ttl: 60_000 });

    expect(result).toEqual({ data: 'fresh', revalidating: null });
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it('does not fall back to cache when a bypassRead fetch fails', async () => {
    mockInvoke.mockResolvedValue('cached');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    mockInvoke.mockRejectedValue(new Error('offline'));

    await expect(
      cachedCommand('cmd', undefined, { ttl: 60_000, bypassRead: true })
    ).rejects.toThrow('offline');
  });

  it('honors epoch invalidation during an in-flight bypassRead fetch', async () => {
    mockInvoke.mockResolvedValue('cached');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    let resolveNetwork!: (value: string) => void;
    const pending = new Promise<string>((resolve) => {
      resolveNetwork = resolve;
    });
    mockInvoke.mockReturnValueOnce(pending);

    const inFlight = cachedCommand<string>('cmd', undefined, {
      ttl: 60_000,
      bypassRead: true,
    });

    // Let cachedCommand reach the network step.
    await Promise.resolve();
    await Promise.resolve();

    await invalidateCache('cmd');

    resolveNetwork('post-invalidate');
    await expect(inFlight).resolves.toEqual({ data: 'post-invalidate', revalidating: null });

    // The epoch bump should have suppressed the write — a subsequent read is a miss.
    mockInvoke.mockResolvedValueOnce('fresh');
    const result = await cachedCommand<string>('cmd', undefined, { ttl: 60_000 });

    expect(result).toEqual({ data: 'fresh', revalidating: null });
  });
});

describe('invalidateCache', () => {
  it('removes a specific entry so next call is a miss', async () => {
    mockInvoke.mockResolvedValue('data');
    await cachedCommand('cmd', { id: '1' }, { ttl: 60_000 });

    await invalidateCache('cmd', { id: '1' });

    mockInvoke.mockResolvedValue('fresh');
    const results = [];
    for await (const r of cachedInvoke('cmd', { id: '1' }, { ttl: 60_000 })) {
      results.push(r);
    }

    // Only network, no cache hit
    expect(results).toEqual([{ data: 'fresh', source: 'network', fetchedAt: expect.any(Number) }]);
  });
});

describe('invalidateCacheByCommand', () => {
  it('removes all entries for a command', async () => {
    mockInvoke.mockResolvedValue('a');
    await cachedCommand('cmd', { id: '1' }, { ttl: 60_000 });
    mockInvoke.mockResolvedValue('b');
    await cachedCommand('cmd', { id: '2' }, { ttl: 60_000 });

    await invalidateCacheByCommand('cmd');

    mockInvoke.mockResolvedValue('fresh');
    const results = [];
    for await (const r of cachedInvoke('cmd', { id: '1' }, { ttl: 60_000 })) {
      results.push(r);
    }
    expect(results).toEqual([{ data: 'fresh', source: 'network', fetchedAt: expect.any(Number) }]);
  });

  it('does not affect other commands', async () => {
    mockInvoke.mockResolvedValue('keep');
    await cachedCommand('other', undefined, { ttl: 60_000 });

    await invalidateCacheByCommand('cmd');

    mockInvoke.mockResolvedValue('new');
    const results = [];
    for await (const r of cachedInvoke('other', undefined, { ttl: 60_000 })) {
      results.push(r);
    }
    // Should still have cache hit (fresh, so no network call)
    expect(results).toEqual([{ data: 'keep', source: 'cache', fetchedAt: expect.any(Number) }]);
  });
});

describe('invalidateCacheByArgs', () => {
  it('removes matching entries for the same command and branchId', async () => {
    mockInvoke.mockResolvedValue('branch-a-head');
    await cachedCommand(
      'get_diff_files',
      { branchId: 'branch-a', commitSha: undefined, scope: 'branch' },
      { ttl: 60_000 }
    );
    mockInvoke.mockResolvedValue('branch-a-commit');
    await cachedCommand(
      'get_diff_files',
      { branchId: 'branch-a', commitSha: 'abc123', scope: 'commit' },
      { ttl: 60_000 }
    );

    await invalidateCacheByArgs('get_diff_files', { branchId: 'branch-a' });

    mockInvoke.mockResolvedValue('fresh');
    const headResults = [];
    for await (const r of cachedInvoke(
      'get_diff_files',
      { branchId: 'branch-a', commitSha: undefined, scope: 'branch' },
      { ttl: 60_000 }
    )) {
      headResults.push(r);
    }

    const commitResults = [];
    for await (const r of cachedInvoke(
      'get_diff_files',
      { branchId: 'branch-a', commitSha: 'abc123', scope: 'commit' },
      { ttl: 60_000 }
    )) {
      commitResults.push(r);
    }

    expect(headResults).toEqual([
      { data: 'fresh', source: 'network', fetchedAt: expect.any(Number) },
    ]);
    expect(commitResults).toEqual([
      { data: 'fresh', source: 'network', fetchedAt: expect.any(Number) },
    ]);
  });

  it('keeps entries for other branches cached', async () => {
    mockInvoke.mockResolvedValue('branch-a');
    await cachedCommand(
      'get_diff_files',
      { branchId: 'branch-a', commitSha: 'abc123', scope: 'commit' },
      { ttl: 60_000 }
    );
    mockInvoke.mockResolvedValue('branch-b');
    await cachedCommand(
      'get_diff_files',
      { branchId: 'branch-b', commitSha: 'def456', scope: 'commit' },
      { ttl: 60_000 }
    );

    await invalidateCacheByArgs('get_diff_files', { branchId: 'branch-a' });

    mockInvoke.mockResolvedValue('fresh');
    const results = [];
    for await (const r of cachedInvoke(
      'get_diff_files',
      { branchId: 'branch-b', commitSha: 'def456', scope: 'commit' },
      { ttl: 60_000 }
    )) {
      results.push(r);
    }

    expect(results).toEqual([{ data: 'branch-b', source: 'cache', fetchedAt: expect.any(Number) }]);
  });

  it('keeps entries for other commands cached', async () => {
    mockInvoke.mockResolvedValue('diff');
    await cachedCommand(
      'get_diff_files',
      { branchId: 'branch-a', commitSha: 'abc123', scope: 'commit' },
      { ttl: 60_000 }
    );
    mockInvoke.mockResolvedValue('messages');
    await cachedCommand('get_session_messages', { branchId: 'branch-a' }, { ttl: 60_000 });

    await invalidateCacheByArgs('get_diff_files', { branchId: 'branch-a' });

    mockInvoke.mockResolvedValue('fresh');
    const results = [];
    for await (const r of cachedInvoke(
      'get_session_messages',
      { branchId: 'branch-a' },
      { ttl: 60_000 }
    )) {
      results.push(r);
    }

    expect(results).toEqual([{ data: 'messages', source: 'cache', fetchedAt: expect.any(Number) }]);
  });

  it('matches entries with optional args missing when branchId matches', async () => {
    mockInvoke.mockResolvedValue('branch-diff');
    await cachedCommand(
      'get_diff_files',
      { branchId: 'branch-a', scope: 'branch' },
      { ttl: 60_000 }
    );

    await invalidateCacheByArgs('get_diff_files', { branchId: 'branch-a' });

    mockInvoke.mockResolvedValue('fresh');
    const results = [];
    for await (const r of cachedInvoke(
      'get_diff_files',
      { branchId: 'branch-a', scope: 'branch' },
      { ttl: 60_000 }
    )) {
      results.push(r);
    }

    expect(results).toEqual([{ data: 'fresh', source: 'network', fetchedAt: expect.any(Number) }]);
  });
});

describe('first-load race protection', () => {
  it('invalidateCacheByCommand blocks a cache write from an in-flight first-load fetch', async () => {
    const pending = deferred<string>();
    mockInvoke.mockReturnValueOnce(pending.promise);

    const inFlight = cachedCommand<string>('list_projects', undefined, { ttl: 60_000 });

    // Let cachedCommand reach the network step (IDB miss, addInFlight, network promise constructed)
    await Promise.resolve();
    await Promise.resolve();

    await invalidateCacheByCommand('list_projects');

    pending.resolve('pre-mutation');
    await expect(inFlight).resolves.toEqual({ data: 'pre-mutation', revalidating: null });

    // The pre-mutation write should have been suppressed by the epoch bump,
    // so a subsequent read must hit the network.
    mockInvoke.mockResolvedValueOnce('fresh');
    const results = [];
    for await (const r of cachedInvoke('list_projects', undefined, { ttl: 60_000 })) {
      results.push(r);
    }
    expect(results).toEqual([{ data: 'fresh', source: 'network', fetchedAt: expect.any(Number) }]);
  });

  it('advances the epoch when consecutive invalidations share a timestamp', async () => {
    const dateNow = vi.spyOn(Date, 'now').mockReturnValue(1_000);
    try {
      // Establish an existing epoch, then begin a fetch under it.
      await invalidateCache('same_millisecond');
      const pending = deferred<string>();
      mockInvoke.mockReturnValueOnce(pending.promise);
      const inFlight = cachedCommand<string>('same_millisecond', undefined, {
        ttl: 60_000,
        bypassRead: true,
      });

      await Promise.resolve();
      await Promise.resolve();

      // Date.now() has not advanced. A timestamp-backed epoch would collide
      // with the first invalidation and allow the stale response to be cached.
      await invalidateCache('same_millisecond');
      pending.resolve('pre-mutation');
      await expect(inFlight).resolves.toEqual({ data: 'pre-mutation', revalidating: null });

      mockInvoke.mockResolvedValueOnce('fresh');
      const followUp = await cachedCommand<string>('same_millisecond', undefined, {
        ttl: 60_000,
      });
      expect(followUp).toEqual({ data: 'fresh', revalidating: null });
    } finally {
      dateNow.mockRestore();
    }
  });

  it('invalidateCacheByArgs blocks a cache write from an in-flight first-load fetch', async () => {
    const pending = deferred<string>();
    mockInvoke.mockReturnValueOnce(pending.promise);

    const inFlight = cachedCommand<string>(
      'get_diff_files',
      { branchId: 'branch-a' },
      { ttl: 60_000 }
    );

    await Promise.resolve();
    await Promise.resolve();

    await invalidateCacheByArgs('get_diff_files', { branchId: 'branch-a' });

    pending.resolve('pre-mutation');
    await expect(inFlight).resolves.toEqual({ data: 'pre-mutation', revalidating: null });

    mockInvoke.mockResolvedValueOnce('fresh');
    const results = [];
    for await (const r of cachedInvoke(
      'get_diff_files',
      { branchId: 'branch-a' },
      { ttl: 60_000 }
    )) {
      results.push(r);
    }
    expect(results).toEqual([{ data: 'fresh', source: 'network', fetchedAt: expect.any(Number) }]);
  });

  it('refcounts in-flight keys so concurrent reads of the same key both honor invalidation', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    mockInvoke.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);

    const callA = cachedCommand<string>('cmd', undefined, { ttl: 60_000 });
    const callB = cachedCommand<string>('cmd', undefined, { ttl: 60_000 });

    await Promise.resolve();
    await Promise.resolve();

    // Resolve the first call; let the cache write settle.
    first.resolve('a');
    await callA;
    await Promise.resolve();
    await Promise.resolve();

    // Refcount should still hold the in-flight key from callB. The invalidator
    // must bump that key's epoch even though IDB now has an entry too.
    await invalidateCacheByCommand('cmd');

    second.resolve('b');
    await callB;

    // Both writes are suppressed (the first by the IDB delete, the second by
    // the epoch bump), so a subsequent read must hit the network.
    mockInvoke.mockResolvedValueOnce('fresh');
    const results = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      results.push(r);
    }
    expect(results).toEqual([{ data: 'fresh', source: 'network', fetchedAt: expect.any(Number) }]);
  });
});

describe('markAllStale', () => {
  it('blocks a pre-gap first-load response from populating the cache', async () => {
    const pending = deferred<string>();
    mockInvoke.mockReturnValueOnce(pending.promise);

    const inFlight = cachedCommand<string>('cmd', undefined, { ttl: 60_000 });
    await vi.waitFor(() => expect(mockInvoke).toHaveBeenCalledTimes(1));

    await markAllStale();
    pending.resolve('pre-gap');
    await expect(inFlight).resolves.toEqual({ data: 'pre-gap', revalidating: null });

    mockInvoke.mockResolvedValueOnce('post-gap');
    await expect(cachedCommand<string>('cmd', undefined, { ttl: 60_000 })).resolves.toEqual({
      data: 'post-gap',
      revalidating: null,
    });
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it('blocks a pre-gap cachedInvoke response from populating the cache', async () => {
    const pending = deferred<string>();
    mockInvoke.mockReturnValueOnce(pending.promise);

    const consume = async () => {
      const results = [];
      for await (const result of cachedInvoke<string>('cmd', undefined, { ttl: 60_000 })) {
        results.push(result);
      }
      return results;
    };
    const inFlight = consume();
    await vi.waitFor(() => expect(mockInvoke).toHaveBeenCalledTimes(1));

    await markAllStale();
    pending.resolve('pre-gap');
    await expect(inFlight).resolves.toEqual([
      { data: 'pre-gap', source: 'network', fetchedAt: expect.any(Number) },
    ]);

    mockInvoke.mockResolvedValueOnce('post-gap');
    await expect(consume()).resolves.toEqual([
      { data: 'post-gap', source: 'network', fetchedAt: expect.any(Number) },
    ]);
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it('yields stale cache first then revalidates from network', async () => {
    mockInvoke.mockResolvedValue('cached');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    await markAllStale();

    mockInvoke.mockResolvedValue('fresh');
    const results = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      results.push(r);
    }

    expect(results).toEqual([
      { data: 'cached', source: 'cache', fetchedAt: expect.any(Number) },
      { data: 'fresh', source: 'network', fetchedAt: expect.any(Number) },
    ]);
  });

  it('preserves original fetchedAt on cache yield and uses fresh timestamp on network yield', async () => {
    mockInvoke.mockResolvedValue('cached');
    const beforePrime = Date.now();
    await cachedCommand('cmd', undefined, { ttl: 60_000 });
    const afterPrime = Date.now();

    // Wait so a new Date.now() is meaningfully later than the prime timestamp
    await new Promise((r) => setTimeout(r, 10));
    await markAllStale();

    mockInvoke.mockResolvedValue('fresh');
    const beforeFetch = Date.now();
    const results = [];
    for await (const r of cachedInvoke<string>('cmd', undefined, { ttl: 60_000 })) {
      results.push(r);
    }

    expect(results).toHaveLength(2);
    const [cacheYield, networkYield] = results;
    expect(cacheYield.source).toBe('cache');
    expect(cacheYield.fetchedAt).toBeGreaterThanOrEqual(beforePrime);
    expect(cacheYield.fetchedAt).toBeLessThanOrEqual(afterPrime);
    expect(networkYield.source).toBe('network');
    expect(networkYield.fetchedAt).toBeGreaterThanOrEqual(beforeFetch);
    expect(networkYield.fetchedAt).toBeGreaterThan(cacheYield.fetchedAt);
  });

  it('cachedCommand returns stale value with a revalidating promise resolving to fresh', async () => {
    mockInvoke.mockResolvedValue('cached');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    await markAllStale();

    mockInvoke.mockResolvedValue('fresh');
    const result = await cachedCommand<string>('cmd', undefined, { ttl: 60_000 });

    expect(result.data).toBe('cached');
    expect(result.revalidating).not.toBeNull();
    await expect(result.revalidating).resolves.toBe('fresh');
  });

  it('revalidation clears the stale flag', async () => {
    mockInvoke.mockResolvedValue('v1');
    await cachedCommand('cmd', undefined, { ttl: 60_000 });

    await markAllStale();

    // Revalidate — await the revalidation promise so the cache write completes
    mockInvoke.mockResolvedValue('v2');
    const { revalidating } = await cachedCommand<string>('cmd', undefined, { ttl: 60_000 });
    await revalidating;

    // Now the entry should be fresh — short-circuit with only cache
    mockInvoke.mockResolvedValue('v3');
    const results = [];
    for await (const r of cachedInvoke('cmd', undefined, { ttl: 60_000 })) {
      results.push(r);
    }

    expect(results).toEqual([{ data: 'v2', source: 'cache', fetchedAt: expect.any(Number) }]);
    expect(mockInvoke).toHaveBeenCalledTimes(2); // v1 + v2, not v3
  });
});

describe('cache maintenance', () => {
  it('does not evict readable entries by count', async () => {
    const total = 250;
    for (let i = 0; i < total; i++) {
      mockInvoke.mockResolvedValue(`value-${i}`);
      await cachedCommand('cmd', { id: String(i) }, { ttl: 60_000 });
    }

    for (let i = 0; i < total; i++) {
      const result = await cachedCommand<string>('cmd', { id: String(i) }, { ttl: 60_000 });
      expect(result).toEqual({ data: `value-${i}`, revalidating: null });
    }
    expect(mockInvoke).toHaveBeenCalledTimes(total);
  });

  it('sweepCache removes stale-schema and week-old entries while keeping fresh ones', async () => {
    const store = createStore('staged-cache', 'responses');
    const freshKey = _cacheKey('fresh');
    const oldKey = _cacheKey('old');
    const staleSchemaKey = _cacheKey('stale_schema');
    const now = Date.now();

    await set(
      freshKey,
      { key: freshKey, data: 'fresh', fetchedAt: now, schemaVersion: CACHE_SCHEMA_VERSION },
      store
    );
    await set(
      oldKey,
      {
        key: oldKey,
        data: 'old',
        fetchedAt: now - 8 * 24 * 60 * 60 * 1000,
        schemaVersion: CACHE_SCHEMA_VERSION,
      },
      store
    );
    await set(
      staleSchemaKey,
      {
        key: staleSchemaKey,
        data: 'stale',
        fetchedAt: now,
        schemaVersion: CACHE_SCHEMA_VERSION - 1,
      },
      store
    );

    await sweepCache();

    const remaining = new Map(await entries<string, unknown>(store));
    expect(remaining.has(freshKey)).toBe(true);
    expect(remaining.has(oldKey)).toBe(false);
    expect(remaining.has(staleSchemaKey)).toBe(false);
  });

  it('evicts the older half and retries once on QuotaExceededError', async () => {
    const store = createStore('staged-cache', 'responses');
    const now = Date.now();
    for (let i = 0; i < 4; i++) {
      const key = _cacheKey('seed', { id: String(i) });
      await set(
        key,
        { key, data: `seed-${i}`, fetchedAt: now + i, schemaVersion: CACHE_SCHEMA_VERSION },
        store
      );
    }

    const originalPut = IDBObjectStore.prototype.put;
    let shouldThrow = true;
    const putSpy = vi.spyOn(IDBObjectStore.prototype, 'put').mockImplementation(function (
      this: IDBObjectStore,
      ...args
    ) {
      if (shouldThrow) {
        shouldThrow = false;
        throw new DOMException('quota exceeded', 'QuotaExceededError');
      }
      return originalPut.apply(this, args as Parameters<IDBObjectStore['put']>);
    });

    try {
      mockInvoke.mockResolvedValue('fresh');
      await cachedCommand('cmd', undefined, { ttl: 60_000 });
    } finally {
      putSpy.mockRestore();
    }

    const remaining = new Map(await entries<string, { data: string }>(store));
    expect(remaining.has(_cacheKey('seed', { id: '0' }))).toBe(false);
    expect(remaining.has(_cacheKey('seed', { id: '1' }))).toBe(false);
    expect(remaining.has(_cacheKey('seed', { id: '2' }))).toBe(true);
    expect(remaining.has(_cacheKey('seed', { id: '3' }))).toBe(true);
    expect(remaining.get(_cacheKey('cmd'))?.data).toBe('fresh');
  });
});
