import { describe, expect, it } from 'vitest';
import { extractPrNumber, extractPrUrl } from './branchCardHelpers';

describe('extractPrUrl', () => {
  it('returns the canonical GitHub PR URL from a PR_URL marker', () => {
    expect(
      extractPrUrl([
        {
          role: 'assistant',
          content: 'PR_URL: https://github.com/block/builderbot/pull/123?expand=1',
        },
      ])
    ).toBe('https://github.com/block/builderbot/pull/123');
  });

  it('ignores placeholder PR_URL markers that do not contain a real PR number', () => {
    expect(
      extractPrUrl([
        {
          role: 'assistant',
          content: 'Return the required line exactly like this: PR_URL: https://github.com/...',
        },
      ])
    ).toBeNull();
  });

  it('strips surrounding markdown punctuation from fallback URL matches', () => {
    expect(
      extractPrUrl([
        {
          role: 'assistant',
          content: 'Created it successfully: <https://github.com/block/builderbot/pull/456>.',
        },
      ])
    ).toBe('https://github.com/block/builderbot/pull/456');
  });
});

describe('extractPrNumber', () => {
  it('reads the PR number from a canonical URL', () => {
    expect(extractPrNumber('https://github.com/block/builderbot/pull/456')).toBe(456);
  });
});
