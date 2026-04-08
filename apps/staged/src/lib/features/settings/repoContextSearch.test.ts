import { describe, expect, it } from 'vitest';
import { matchesRepoContextSearch } from './repoContextSearch';
import type { ActionContext } from '../../api/commands';

const context: ActionContext = {
  id: 'ctx-1',
  githubRepo: 'block/builderbot',
  subpath: 'apps/staged',
  hasDetectedActions: false,
  detectingActions: false,
  createdAt: 0,
  updatedAt: 0,
};

describe('matchesRepoContextSearch', () => {
  it('matches tokens across repo and subpath fields', () => {
    expect(matchesRepoContextSearch(context, 'staged block')).toBe(true);
  });

  it('matches tokens regardless of order', () => {
    expect(matchesRepoContextSearch(context, 'builderbot apps')).toBe(true);
  });

  it('matches case-insensitively', () => {
    expect(matchesRepoContextSearch(context, 'BLOCK STAGED')).toBe(true);
  });

  it('handles missing subpaths', () => {
    expect(
      matchesRepoContextSearch(
        {
          ...context,
          subpath: null,
        },
        'builderbot'
      )
    ).toBe(true);
  });

  it('requires every token to match some repo term', () => {
    expect(matchesRepoContextSearch(context, 'staged unknown')).toBe(false);
  });
});
