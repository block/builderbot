import { describe, expect, it } from 'vitest';
import { buildGithubCommentUrl } from './diffModalHelpers';

describe('buildGithubCommentUrl', () => {
  it('builds inline review comment URLs from a PR URL', () => {
    expect(
      buildGithubCommentUrl(
        { githubCommentId: 12345, githubCommentType: 'review' },
        { prUrl: 'https://github.com/block/builderbot/pull/42' }
      )
    ).toBe('https://github.com/block/builderbot/pull/42#discussion_r12345');
  });

  it('builds issue comment fallback URLs from repo and PR number', () => {
    expect(
      buildGithubCommentUrl(
        { githubCommentId: 67890, githubCommentType: 'issue' },
        { githubRepo: 'block/builderbot', prNumber: 42 }
      )
    ).toBe('https://github.com/block/builderbot/pull/42#issuecomment-67890');
  });

  it('removes any existing PR URL fragment before adding the comment anchor', () => {
    expect(
      buildGithubCommentUrl(
        { githubCommentId: 12345, githubCommentType: 'review' },
        { prUrl: 'https://github.com/block/builderbot/pull/42#files' }
      )
    ).toBe('https://github.com/block/builderbot/pull/42#discussion_r12345');
  });

  it('returns null when the synced comment data is incomplete', () => {
    expect(
      buildGithubCommentUrl(
        { githubCommentId: null, githubCommentType: 'review' },
        { prUrl: 'https://github.com/block/builderbot/pull/42' }
      )
    ).toBeNull();
  });
});
