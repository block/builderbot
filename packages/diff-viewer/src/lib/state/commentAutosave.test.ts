import { afterEach, describe, expect, it, vi } from 'vitest';
import { createCommentAutosaveController } from './commentAutosave';
import type { Comment } from '../types';

function createComment(overrides: Partial<Comment> = {}): Comment {
  return {
    id: 'comment-1',
    path: 'src/app.ts',
    span: { start: 1, end: 2 },
    content: 'Saved content',
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

describe('createCommentAutosaveController', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('debounces the first add and adopts the returned comment for later updates', async () => {
    vi.useFakeTimers();
    const addComment = vi.fn(async (content: string) => createComment({ content }));
    const updateComment = vi.fn(async () => {});
    const controller = createCommentAutosaveController({
      debounceMs: 500,
      addComment,
      updateComment,
    });

    controller.setContent('First draft');
    await vi.advanceTimersByTimeAsync(499);

    expect(addComment).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);

    expect(addComment).toHaveBeenCalledWith('First draft');
    expect(controller.getSnapshot().comment?.id).toBe('comment-1');
    expect(controller.getSnapshot().status).toBe('saved');

    controller.setContent('Updated draft');
    await vi.advanceTimersByTimeAsync(500);

    expect(updateComment).toHaveBeenCalledWith('comment-1', 'Updated draft');
    expect(controller.getSnapshot().comment?.content).toBe('Updated draft');
  });

  it('flushes pending debounce work immediately before an action uses the comment', async () => {
    vi.useFakeTimers();
    const addComment = vi.fn(async (content: string) => createComment({ content }));
    const controller = createCommentAutosaveController({
      debounceMs: 500,
      addComment,
      updateComment: vi.fn(async () => {}),
    });

    controller.setContent('Action draft');
    const savedComment = await controller.flush();

    expect(addComment).toHaveBeenCalledWith('Action draft');
    expect(savedComment?.content).toBe('Action draft');
  });

  it('serializes saves so newer text is updated after an older add resolves', async () => {
    let resolveAdd: (comment: Comment) => void = () => {};
    const addComment = vi.fn(
      (content: string) =>
        new Promise<Comment>((resolve) => {
          resolveAdd = resolve;
        })
    );
    const updateComment = vi.fn(async () => {});
    const controller = createCommentAutosaveController({
      addComment,
      updateComment,
    });

    controller.setContent('Old text');
    const flushPromise = controller.flush();

    expect(addComment).toHaveBeenCalledWith('Old text');

    controller.setContent('New text');
    resolveAdd(createComment({ id: 'comment-1', content: 'Old text' }));
    const savedComment = await flushPromise;

    expect(updateComment).toHaveBeenCalledWith('comment-1', 'New text');
    expect(savedComment?.content).toBe('New text');
    expect(controller.getSnapshot().comment?.content).toBe('New text');
  });

  it('closes empty never-saved drafts without creating a comment', async () => {
    const addComment = vi.fn(async (content: string) => createComment({ content }));
    const controller = createCommentAutosaveController({
      addComment,
      updateComment: vi.fn(async () => {}),
    });

    controller.setContent('   ');

    await expect(controller.flush()).resolves.toBeNull();
    expect(addComment).not.toHaveBeenCalled();
    expect(controller.getSnapshot().status).toBe('idle');
  });
});
