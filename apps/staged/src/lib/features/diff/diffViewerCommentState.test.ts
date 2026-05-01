import { describe, expect, it } from 'vitest';
import { resolveTrackedComment } from '@builderbot/diff-viewer/utils';
import type { Comment } from '../../types';

function createComment(overrides: Partial<Comment> = {}): Comment {
  return {
    id: 'comment-1',
    path: 'src/app.ts',
    span: { start: 2, end: 3 },
    content: 'Check this line',
    author: 'user',
    commentType: 'suggestion',
    createdAt: 1,
    deletedAt: null,
    ...overrides,
  };
}

describe('resolveTrackedComment', () => {
  it('uses the latest comments prop for a jump-opened comment', () => {
    const openedComment = createComment({ content: 'Old content' });
    const latestComment = createComment({ content: 'Updated content' });

    expect(resolveTrackedComment([latestComment], openedComment, null)).toEqual({
      commentId: 'comment-1',
      existingComment: latestComment,
      missing: false,
    });
  });

  it('marks a jump-opened comment as missing after rerender removes it', () => {
    const openedComment = createComment();

    expect(resolveTrackedComment([], openedComment, null)).toEqual({
      commentId: 'comment-1',
      existingComment: null,
      missing: true,
    });
  });

  it('keeps a new comment editor open when there is no tracked comment id', () => {
    expect(resolveTrackedComment([], null, null)).toEqual({
      commentId: null,
      existingComment: null,
      missing: false,
    });
  });
});
