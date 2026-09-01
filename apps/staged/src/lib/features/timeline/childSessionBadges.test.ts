import { describe, expect, it } from 'vitest';
import type { ChildSessionInfo, RepoBadge } from '../../types';
import {
  buildChildSessionBadges,
  childSessionRepos,
  fallbackShortName,
  runningSecondaryMeta,
} from './childSessionBadges';

function child(overrides: Partial<ChildSessionInfo> = {}): ChildSessionInfo {
  return {
    sessionId: 'session-1',
    sessionType: 'commit',
    branchId: 'branch-1',
    githubRepo: 'block/staged',
    subpath: null,
    ...overrides,
  };
}

function repoBadge(overrides: Partial<RepoBadge> = {}): RepoBadge {
  return {
    githubRepo: 'block/staged',
    subpath: '',
    shortName: 'staged',
    hue: 210,
    createdAt: 0,
    pinned: false,
    pinSortOrder: null,
    defaultBranch: null,
    ...overrides,
  };
}

/** Badge cache stocked with the given badges, keyed the way the store keys them. */
function lookup(badges: RepoBadge[]) {
  const byKey = new Map(badges.map((badge) => [`${badge.githubRepo}:${badge.subpath}`, badge]));
  return (githubRepo: string, subpath: string | null) =>
    byKey.get(`${githubRepo}:${subpath ?? ''}`);
}

describe('fallbackShortName', () => {
  it('uses the repo name after the owner', () => {
    expect(fallbackShortName('block/builderbot', null)).toBe('builderbot');
  });

  it('prefers the subpath tail for a monorepo entry', () => {
    expect(fallbackShortName('block/builderbot', 'apps/staged')).toBe('staged');
  });

  it('ignores a blank subpath', () => {
    expect(fallbackShortName('block/builderbot', '  ')).toBe('builderbot');
  });

  it('has nothing to name without a repo', () => {
    expect(fallbackShortName(null, null)).toBeNull();
  });
});

describe('buildChildSessionBadges', () => {
  it('tints each badge with its repo hue', () => {
    const badges = buildChildSessionBadges(
      [
        child({ sessionId: 'a', sessionType: 'commit', githubRepo: 'block/staged' }),
        child({ sessionId: 'b', sessionType: 'note', githubRepo: 'block/acp' }),
      ],
      lookup([repoBadge(), repoBadge({ githubRepo: 'block/acp', shortName: 'acp', hue: 40 })])
    );

    expect(badges).toEqual([
      { key: 'a', sessionType: 'commit', shortName: 'staged', hue: 210 },
      { key: 'b', sessionType: 'note', shortName: 'acp', hue: 40 },
    ]);
  });

  it('matches a subpath badge on its subpath', () => {
    const badges = buildChildSessionBadges(
      [child({ subpath: 'apps/staged' })],
      lookup([
        repoBadge({ subpath: '', shortName: 'wholerepo', hue: 10 }),
        repoBadge({ subpath: 'apps/staged', shortName: 'staged', hue: 210 }),
      ])
    );

    expect(badges[0]).toMatchObject({ shortName: 'staged', hue: 210 });
  });

  it('keeps a child visible with a fallback name while its badge generates', () => {
    const badges = buildChildSessionBadges([child({ githubRepo: 'block/builderbot' })], lookup([]));

    expect(badges).toEqual([
      { key: 'session-1', sessionType: 'commit', shortName: 'builderbot', hue: null },
    ]);
  });

  it('renders a repo-less child as a bare type badge', () => {
    const badges = buildChildSessionBadges(
      [child({ githubRepo: null, subpath: null, sessionType: 'review' })],
      lookup([repoBadge()])
    );

    expect(badges).toEqual([
      { key: 'session-1', sessionType: 'review', shortName: null, hue: null },
    ]);
  });

  it('gives two children in the same repo their own badges', () => {
    const badges = buildChildSessionBadges(
      [child({ sessionId: 'a' }), child({ sessionId: 'b', sessionType: 'note' })],
      lookup([repoBadge()])
    );

    expect(badges.map((badge) => badge.key)).toEqual(['a', 'b']);
    expect(badges.map((badge) => badge.sessionType)).toEqual(['commit', 'note']);
  });
});

describe('childSessionRepos', () => {
  it('deduplicates repos and skips repo-less children', () => {
    expect(
      childSessionRepos([
        child({ sessionId: 'a' }),
        child({ sessionId: 'b' }),
        child({ sessionId: 'c', subpath: 'apps/staged' }),
        child({ sessionId: 'd', githubRepo: null }),
      ])
    ).toEqual([
      { githubRepo: 'block/staged', subpath: null },
      { githubRepo: 'block/staged', subpath: 'apps/staged' },
    ]);
  });
});

describe('runningSecondaryMeta', () => {
  it('shows the live hint when nothing is in flight', () => {
    expect(runningSecondaryMeta([], 'Reading the migration')).toBe('Reading the migration');
  });

  it('falls back to a placeholder before the first hint arrives', () => {
    expect(runningSecondaryMeta([], undefined)).toBe('Generating note');
  });

  it('yields the meta line to child badges', () => {
    const badges = buildChildSessionBadges([child()], lookup([repoBadge()]));
    expect(runningSecondaryMeta(badges, 'Reading the migration')).toBeUndefined();
  });
});
