import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RepoBadge } from '../types';

const { badges, store } = vi.hoisted(() => ({
  /** Badges by `githubRepo:subpath`, as the store keys them. */
  badges: new Map<string, RepoBadge>(),
  /** What a lookup answers for a repo without a badge. */
  store: { missing: undefined as null | undefined },
}));

vi.mock('../stores/repoBadges.svelte', () => ({
  repoBadgeStore: {
    lookup: (githubRepo: string, subpath: string | null | undefined) =>
      badges.get(`${githubRepo}:${subpath ?? ''}`) ?? store.missing,
  },
}));

import { repoFullName, repoShortLabel } from './repoShortLabel';

function addBadge(githubRepo: string, subpath: string, shortName: string, hue: number) {
  badges.set(`${githubRepo}:${subpath}`, {
    githubRepo,
    subpath,
    shortName,
    hue,
    createdAt: 0,
    pinned: false,
    pinSortOrder: null,
    defaultBranch: 'main',
  });
}

beforeEach(() => {
  badges.clear();
  store.missing = undefined;
});

describe('repoShortLabel', () => {
  it('is the short name and hue of the repo’s badge', () => {
    addBadge('block/goose', '', 'goose', 20);

    expect(repoShortLabel({ githubRepo: 'block/goose' })).toEqual({ shortName: 'goose', hue: 20 });
  });

  it('treats a missing, null or empty subpath as the repo itself', () => {
    addBadge('block/goose', '', 'goose', 20);

    expect(repoShortLabel({ githubRepo: 'block/goose', subpath: null })?.shortName).toBe('goose');
    expect(repoShortLabel({ githubRepo: 'block/goose', subpath: '' })?.shortName).toBe('goose');
  });

  it('gives a monorepo subpath its own short name, not the repo’s', () => {
    addBadge('block/builderbot', '', 'bb', 200);
    addBadge('block/builderbot', 'apps/staged', 'staged', 120);

    expect(repoShortLabel({ githubRepo: 'block/builderbot' })?.shortName).toBe('bb');
    expect(repoShortLabel({ githubRepo: 'block/builderbot', subpath: 'apps/staged' })).toEqual({
      shortName: 'staged',
      hue: 120,
    });
  });

  it('keeps two repos with the same name apart by their badges’ names', () => {
    // The backend dedups on insert; the label shows whatever it stored.
    addBadge('block/goose', '', 'goose', 20);
    addBadge('fork/goose', '', 'goose2', 140);

    expect(repoShortLabel({ githubRepo: 'block/goose' })?.shortName).toBe('goose');
    expect(repoShortLabel({ githubRepo: 'fork/goose' })?.shortName).toBe('goose2');
  });

  it('is undefined for a repo without a badge, rather than a derived name', () => {
    addBadge('block/builderbot', '', 'bb', 200);

    expect(repoShortLabel({ githubRepo: 'block/goose' })).toBeUndefined();
    expect(repoShortLabel({ githubRepo: 'block/builderbot', subpath: 'apps/web' })).toBeUndefined();
  });

  it('is undefined when the store answers null, as the component tests’ mocks do', () => {
    store.missing = null;

    expect(repoShortLabel({ githubRepo: 'nobody/nothing' })).toBeUndefined();
  });
});

describe('repoFullName', () => {
  it('is the repo slug without a subpath', () => {
    expect(repoFullName({ githubRepo: 'block/goose' })).toBe('block/goose');
    expect(repoFullName({ githubRepo: 'block/goose', subpath: null })).toBe('block/goose');
    expect(repoFullName({ githubRepo: 'block/goose', subpath: '' })).toBe('block/goose');
  });

  it('appends the subpath whole', () => {
    expect(repoFullName({ githubRepo: 'block/builderbot', subpath: 'apps/staged' })).toBe(
      'block/builderbot/apps/staged'
    );
  });
});
