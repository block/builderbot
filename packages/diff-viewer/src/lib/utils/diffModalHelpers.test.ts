import { describe, expect, it } from 'vitest';
import type { Comment } from '../types';
import {
  buildFileEntries,
  fileChangeScale,
  fileChangeTotal,
  resolveCommentFilePath,
} from './diffModalHelpers';

// A root file plus two same-named files in subdirectories, in the byte order
// git lists them: `.github/VISION.md` sorts before the root `VISION.md`.
const SAME_NAME_FILES = ['.github/VISION.md', 'VISION.md', 'apps/web/VISION.md'];

function comment(path: string, overrides: Partial<Comment> = {}): Comment {
  return {
    id: `comment-${path}`,
    path,
    span: { start: 0, end: 1 },
    content: 'note',
    author: 'user',
    commentType: 'suggestion',
    createdAt: 0,
    deletedAt: null,
    githubCommentId: null,
    githubCommentType: null,
    githubCommentStale: false,
    noteSessionId: null,
    commitSessionId: null,
    ...overrides,
  };
}

describe('resolveCommentFilePath', () => {
  describe('without a subpath', () => {
    const files = new Set(SAME_NAME_FILES);

    it('keeps a root comment on the root file', () => {
      expect(resolveCommentFilePath('VISION.md', files)).toBe('VISION.md');
      expect(resolveCommentFilePath('VISION.md', files, null)).toBe('VISION.md');
      expect(resolveCommentFilePath('VISION.md', files, '')).toBe('VISION.md');
    });

    it('keeps each subdirectory comment on its own file', () => {
      expect(resolveCommentFilePath('apps/web/VISION.md', files)).toBe('apps/web/VISION.md');
      expect(resolveCommentFilePath('.github/VISION.md', files)).toBe('.github/VISION.md');
    });

    it('does not suffix-match when the root file is absent', () => {
      const subdirOnly = new Set(['apps/web/VISION.md', 'docs/VISION.md']);
      expect(resolveCommentFilePath('VISION.md', subdirOnly)).toBe('VISION.md');
      expect(resolveCommentFilePath('web/VISION.md', subdirOnly)).toBe('web/VISION.md');
    });

    it('does not prefix-strip onto a same-named root file', () => {
      const rootOnly = new Set(['VISION.md']);
      expect(resolveCommentFilePath('docs/VISION.md', rootOnly)).toBe('docs/VISION.md');
    });
  });

  describe('with a subpath', () => {
    it('prefers an exact root-relative match over the subpath candidate', () => {
      const files = new Set([...SAME_NAME_FILES, 'apps/web/apps/web/VISION.md']);
      expect(resolveCommentFilePath('VISION.md', files, 'apps/web')).toBe('VISION.md');
      expect(resolveCommentFilePath('apps/web/VISION.md', files, 'apps/web')).toBe(
        'apps/web/VISION.md'
      );
      expect(resolveCommentFilePath('.github/VISION.md', files, 'apps/web')).toBe(
        '.github/VISION.md'
      );
    });

    it('maps a subpath-relative path onto the root-relative diff file', () => {
      const files = new Set(['apps/web/src/foo.ts', 'docs/VISION.md']);
      expect(resolveCommentFilePath('src/foo.ts', files, 'apps/web')).toBe('apps/web/src/foo.ts');
    });

    it('lands a subpath-relative comment on the subpath file, not a same-named sibling', () => {
      const files = new Set(['apps/web/VISION.md', 'docs/VISION.md', 'apps/api/VISION.md']);
      expect(resolveCommentFilePath('VISION.md', files, 'apps/web')).toBe('apps/web/VISION.md');
    });

    it('strips exactly the subpath prefix in the reverse direction', () => {
      const files = new Set(['src/foo.ts']);
      expect(resolveCommentFilePath('apps/web/src/foo.ts', files, 'apps/web')).toBe('src/foo.ts');
      expect(resolveCommentFilePath('apps/webby/src/foo.ts', files, 'apps/web')).toBe(
        'apps/webby/src/foo.ts'
      );
    });

    it('normalizes surrounding slashes and whitespace on the subpath', () => {
      const files = new Set(['apps/web/src/foo.ts']);
      expect(resolveCommentFilePath('src/foo.ts', files, ' /apps/web/ ')).toBe(
        'apps/web/src/foo.ts'
      );
    });

    it('only tries the configured subpath, never an arbitrary suffix', () => {
      const files = new Set(['docs/VISION.md', 'packages/web/src/foo.ts']);
      expect(resolveCommentFilePath('VISION.md', files, 'apps/web')).toBe('VISION.md');
      expect(resolveCommentFilePath('src/foo.ts', files, 'apps/web')).toBe('src/foo.ts');
    });

    it('leaves the path unchanged when the fallback is ambiguous', () => {
      // Neither the comment path itself nor a unique fallback exists: both
      // `<subpath>/<path>` and `<path>` minus the subpath are diff files.
      const files = new Set(['apps/web/apps/web/foo.ts', 'foo.ts']);
      expect(resolveCommentFilePath('apps/web/foo.ts', files, 'apps/web')).toBe('apps/web/foo.ts');
    });
  });
});

describe('fileChangeTotal', () => {
  it('returns null when a summary has no line stats', () => {
    expect(fileChangeTotal({ before: 'a.ts', after: 'a.ts' })).toBeNull();
  });

  it('adds available added and deleted counts', () => {
    expect(fileChangeTotal({ before: 'a.ts', after: 'a.ts', addedLines: 5, deletedLines: 2 })).toBe(
      7
    );
  });
});

describe('fileChangeScale', () => {
  it('returns 0 when totals are missing or empty', () => {
    expect(fileChangeScale(null, 10)).toBe(0);
    expect(fileChangeScale(0, 10)).toBe(0);
    expect(fileChangeScale(10, 0)).toBe(0);
  });

  it('uses logarithmic scaling against the largest file total', () => {
    expect(fileChangeScale(9, 99)).toBeCloseTo(Math.log1p(9) / Math.log1p(99));
    expect(fileChangeScale(99, 99)).toBe(1);
  });
});

describe('buildFileEntries', () => {
  it('propagates optional line stats to file entries', () => {
    const entries = buildFileEntries(
      [{ before: 'src/file.ts', after: 'src/file.ts', addedLines: 3, deletedLines: 1 }],
      [],
      []
    );

    expect(entries[0]).toMatchObject({
      path: 'src/file.ts',
      addedLines: 3,
      deletedLines: 1,
    });
  });

  it('keeps missing line stats as null for older cached summaries', () => {
    const entries = buildFileEntries([{ before: 'src/file.ts', after: 'src/file.ts' }], [], []);

    expect(entries[0]).toMatchObject({
      addedLines: null,
      deletedLines: null,
    });
  });

  it('counts comments only on the file whose path matches exactly', () => {
    const files = SAME_NAME_FILES.map((path) => ({ before: path, after: path }));
    const entries = buildFileEntries(
      files,
      [],
      [
        comment('VISION.md', { id: 'root-1', commentType: 'issue' }),
        comment('VISION.md', { id: 'root-2' }),
        comment('.github/VISION.md', { id: 'github-1', commentType: 'warning' }),
      ]
    );

    expect(entries.map((e) => [e.path, e.commentCount, e.commentTypes])).toEqual([
      ['.github/VISION.md', 1, ['warning']],
      ['VISION.md', 2, ['issue', 'suggestion']],
      ['apps/web/VISION.md', 0, []],
    ]);
  });
});
