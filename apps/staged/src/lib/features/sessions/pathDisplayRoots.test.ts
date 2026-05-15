import { beforeEach, describe, expect, it } from 'vitest';
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
});
