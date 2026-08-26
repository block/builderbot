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

  it('rejects addComment with the original persistence failure', async () => {
    vi.stubGlobal('$state', <T>(value: T) => value);
    const { createReviewState } = await import('./reviewState.svelte');
    const failure = new Error('add failed');
    const commands: ReviewCommands & Pick<DiffCommands, 'getFileAtRef'> = {
      ensureReview: vi.fn(async () => createReview()),
      findReview: vi.fn(async () => null),
      getReview: vi.fn(async () => null),
      addComment: vi.fn(async () => {
        throw failure;
      }),
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

    await expect(
      reviewState.addComment('src/app.ts', { start: 2, end: 3 }, 'Check this line')
    ).rejects.toThrow(failure);
    expect(reviewState.state.comments).toEqual([]);
    vi.unstubAllGlobals();
  });

  it('rejects updateComment when persistence fails', async () => {
    vi.stubGlobal('$state', <T>(value: T) => value);
    const { createReviewState } = await import('./reviewState.svelte');
    const existingComment = createComment();
    const failure = new Error('write failed');
    const commands: ReviewCommands & Pick<DiffCommands, 'getFileAtRef'> = {
      ensureReview: vi.fn(async () => createReview({ comments: [existingComment] })),
      findReview: vi.fn(async () => createReview({ comments: [existingComment] })),
      getReview: vi.fn(async () => null),
      addComment: vi.fn(async () => existingComment),
      updateComment: vi.fn(async () => {
        throw failure;
      }),
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
    await Promise.resolve();

    await expect(reviewState.updateComment('comment-1', 'Unsaved text')).rejects.toThrow(failure);
    expect(reviewState.state.comments[0]?.content).toBe('Unsaved text');
    vi.unstubAllGlobals();
  });
});
