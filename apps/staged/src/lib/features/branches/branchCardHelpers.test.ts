import { describe, expect, it } from 'vitest';
import { extractPrNumber, extractPrUrl, isImageFile, isMaybeTextFile } from './branchCardHelpers';

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

describe('file type helpers', () => {
  it('recognizes backend-supported image extensions', () => {
    for (const extension of ['png', 'jpg', 'jpeg', 'gif', 'webp']) {
      const filePath = `/tmp/image.${extension}`;

      expect(isImageFile(filePath)).toBe(true);
      expect(isMaybeTextFile(filePath)).toBe(false);
    }
  });

  it('treats unsupported image formats as maybe-text files', () => {
    for (const extension of ['svg', 'bmp', 'tiff', 'ico', 'heic', 'avif']) {
      const filePath = `/tmp/image.${extension}`;

      expect(isImageFile(filePath)).toBe(false);
      expect(isMaybeTextFile(filePath)).toBe(true);
    }
  });
});
