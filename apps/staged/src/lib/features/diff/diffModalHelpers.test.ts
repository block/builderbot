import { describe, expect, it } from 'vitest';
import { buildGithubCommentUrl, cssCustomPropertyDeclarations } from './diffModalHelpers';

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

describe('cssCustomPropertyDeclarations', () => {
  it('is empty for an empty map', () => {
    expect(cssCustomPropertyDeclarations({})).toBe('');
  });

  it('serialises each variable as a declaration, in insertion order', () => {
    expect(
      cssCustomPropertyDeclarations({
        '--bg-primary': '#ffffff',
        '--fg-primary': '#24292f',
        '--diff-add-bg': '#e6ffec',
      })
    ).toBe('--bg-primary: #ffffff;--fg-primary: #24292f;--diff-add-bg: #e6ffec;');
  });

  it('keeps values containing spaces and commas intact', () => {
    expect(
      cssCustomPropertyDeclarations({
        '--font-mono': "'SF Mono', Menlo, monospace",
        '--shadow': '0 1px 2px rgba(0, 0, 0, 0.2)',
      })
    ).toBe("--font-mono: 'SF Mono', Menlo, monospace;--shadow: 0 1px 2px rgba(0, 0, 0, 0.2);");
  });
});
