import { describe, expect, it, vi } from 'vitest';
import type { Comment, DiffCommands, Review, ReviewCommands } from '../types';

function createComment(overrides: Partial<Comment> = {}): Comment {
  return {
    id: 'comment-1',
    path: 'src/app.ts',
    span: { start: 2, end: 3 },
    content: 'Check this line',
    author: 'user',
    commentType: null,
    createdAt: 1,
    deletedAt: null,
    githubCommentId: null,
    githubCommentType: null,
    githubCommentStale: false,
    noteSessionId: null,
    commitSessionId: null,
    ...overrides,
  };
}

function createReview(overrides: Partial<Review> = {}): Review {
  return {
    id: 'review-1',
    branchId: 'branch-1',
    commitSha: 'abc123',
    scope: 'branch',
    sessionId: null,
    reviewed: [],
    comments: [],
    referenceFiles: [],
    createdAt: 1,
    updatedAt: 1,
    completedAt: null,
    sessionProvider: null,
    ...overrides,
  };
}

describe('createReviewState', () => {
  it('returns the created comment from addComment', async () => {
    vi.stubGlobal('$state', <T>(value: T) => value);
    const { createReviewState } = await import('./reviewState.svelte');
    const createdComment = createComment();
    const commands: ReviewCommands & Pick<DiffCommands, 'getFileAtRef'> = {
      ensureReview: vi.fn(async () => createReview()),
      findReview: vi.fn(async () => null),
      getReview: vi.fn(async () => null),
      addComment: vi.fn(async () => createdComment),
      updateComment: vi.fn(async () => {}),
      deleteComment: vi.fn(async () => {}),
      deleteAllComments: vi.fn(async () => {}),
      restoreComment: vi.fn(async () => {}),
      getDeletedComments: vi.fn(async () => []),
      markReviewed: vi.fn(async () => {}),
      unmarkReviewed: vi.fn(async () => {}),
      addReferenceFile: vi.fn(async () => {}),
      removeReferenceFile: vi.fn(async () => {}),
      getFileAtRef: vi.fn(async () => ({
        path: 'src/app.ts',
        content: { type: 'Text', lines: [] },
      })),
    };
    const reviewState = createReviewState(commands, 'branch-1', 'abc123', 'branch');

    const result = await reviewState.addComment(
      'src/app.ts',
      { start: 2, end: 3 },
      'Check this line'
    );

    expect(result).toEqual(createdComment);
    expect(reviewState.state.comments).toEqual([createdComment]);
    vi.unstubAllGlobals();
  });
});
