import { describe, expect, it } from 'vitest';
import type { RepoHomeItem } from '../../types';
import { moveRepo, repoKey } from './repoReorder';

function repo(githubRepo: string, subpath = ''): RepoHomeItem {
  return {
    githubRepo,
    subpath,
    shortName: githubRepo.split('/')[1],
    hue: 0,
    createdAt: 0,
    pinned: true,
    pinSortOrder: null,
    defaultBranch: 'main',
    hasLocalClone: true,
  };
}

const staged = repo('block/builderbot', 'apps/staged');
const differ = repo('block/builderbot', 'apps/differ');
const g2 = repo('squareup/g2');
const pinned = [staged, differ, g2];

describe('repoKey', () => {
  it('distinguishes subpaths of the same github repo', () => {
    expect(repoKey(staged)).not.toBe(repoKey(differ));
  });

  it('is stable across object identity', () => {
    expect(repoKey(repo('squareup/g2'))).toBe(repoKey(g2));
  });
});

describe('moveRepo', () => {
  it('moves the source before an earlier target', () => {
    expect(moveRepo(pinned, repoKey(g2), repoKey(staged))).toEqual([g2, staged, differ]);
  });

  it('moves the source to a later target position', () => {
    expect(moveRepo(pinned, repoKey(staged), repoKey(g2))).toEqual([differ, g2, staged]);
  });

  it('does not mutate the input', () => {
    moveRepo(pinned, repoKey(g2), repoKey(staged));
    expect(pinned).toEqual([staged, differ, g2]);
  });

  it('is a no-op when dropped on itself', () => {
    expect(moveRepo(pinned, repoKey(g2), repoKey(g2))).toBeNull();
  });

  it('is a no-op when the source left the list mid-drag', () => {
    // g2 was unpinned between dragstart and drop.
    const shrunk = [staged, differ];
    expect(moveRepo(shrunk, repoKey(g2), repoKey(staged))).toBeNull();
  });

  it('is a no-op when the target left the list mid-drag', () => {
    const shrunk = [staged, g2];
    expect(moveRepo(shrunk, repoKey(g2), repoKey(differ))).toBeNull();
  });
});
