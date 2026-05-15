import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  clearDisplayRootAliasCacheForTests,
  displayRootKey,
  resolveDisplayRoots,
  type PathAliasResolver,
} from './pathDisplayRoots';

describe('pathDisplayRoots', () => {
  beforeEach(() => {
    clearDisplayRootAliasCacheForTests();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('computes a stable root key from sorted normalized roots', () => {
    expect(displayRootKey(['/tmp/b/', '/tmp/a', '/tmp/a/'])).toBe('/tmp/a\n/tmp/b');
  });

  it('caches alias resolution by normalized root set key', async () => {
    const calls: string[][] = [];
    const resolver: PathAliasResolver = async (paths) => {
      calls.push(paths);
      return [paths[0], `/real${paths[0]}`];
    };

    const first = resolveDisplayRoots(['/tmp/b/', '/tmp/a'], resolver);
    const second = resolveDisplayRoots(['/tmp/a/', '/tmp/b'], resolver);

    expect(second).toBe(first);
    await expect(first).resolves.toEqual(['/tmp/b', '/real/tmp/b', '/tmp/a', '/real/tmp/a']);
    expect(calls).toEqual([['/tmp/b'], ['/tmp/a']]);
  });

  it('retries fallback-only aliases after a short cache window', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-01-01T00:00:00Z'));

    const calls: string[][] = [];
    const resolver: PathAliasResolver = async (paths) => {
      calls.push(paths);
      if (calls.length === 1) {
        return [paths[0]];
      }
      return [paths[0], `/real${paths[0]}`];
    };

    await expect(resolveDisplayRoots('/tmp/worktree', resolver)).resolves.toEqual([
      '/tmp/worktree',
    ]);
    await expect(resolveDisplayRoots('/tmp/worktree', resolver)).resolves.toEqual([
      '/tmp/worktree',
    ]);
    expect(calls).toEqual([['/tmp/worktree']]);

    vi.advanceTimersByTime(5_001);

    await expect(resolveDisplayRoots('/tmp/worktree', resolver)).resolves.toEqual([
      '/tmp/worktree',
      '/real/tmp/worktree',
    ]);
    expect(calls).toEqual([['/tmp/worktree'], ['/tmp/worktree']]);
  });
});
