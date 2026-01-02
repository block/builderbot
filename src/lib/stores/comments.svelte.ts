/**
 * Comments Store
 *
 * Manages comment state for the current diff view.
 * Comments are loaded when a diff is selected and persisted via the review API.
 */

import type { Comment, Span, NewComment } from '../types';
import {
  getReview,
  addComment as apiAddComment,
  updateComment as apiUpdateComment,
  deleteComment as apiDeleteComment,
  exportReviewMarkdown,
} from '../services/review';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';

// =============================================================================
// State
// =============================================================================

interface CommentsState {
  /** All comments for the current diff */
  comments: Comment[];
  /** Currently selected file path (for filtering) */
  currentPath: string | null;
  /** Diff refs for API calls */
  diffBase: string | null;
  diffHead: string | null;
  /** Loading state */
  loading: boolean;
}

export const commentsState: CommentsState = $state({
  comments: [],
  currentPath: null,
  diffBase: null,
  diffHead: null,
  loading: false,
});

// =============================================================================
// Derived
// =============================================================================

/**
 * Comments for the current file only.
 */
export function getCommentsForCurrentFile(): Comment[] {
  if (!commentsState.currentPath) return [];
  return commentsState.comments.filter((c) => c.path === commentsState.currentPath);
}

/**
 * Get comments for a specific line.
 */
export function getCommentsForLine(lineIndex: number): Comment[] {
  return getCommentsForCurrentFile().filter((c) => {
    // Single line comment: span.end === span.start + 1
    return c.span.start === lineIndex && c.span.end === lineIndex + 1;
  });
}

/**
 * Get comments for a specific range (alignment).
 */
export function getCommentsForRange(start: number, end: number): Comment[] {
  return getCommentsForCurrentFile().filter((c) => {
    // Check if spans overlap
    return c.span.start < end && c.span.end > start;
  });
}

/**
 * Find a comment with an exact matching span.
 */
export function findCommentBySpan(span: Span): Comment | undefined {
  return getCommentsForCurrentFile().find(
    (c) => c.span.start === span.start && c.span.end === span.end
  );
}

/**
 * Find a comment by ID.
 */
export function findCommentById(id: string): Comment | undefined {
  return commentsState.comments.find((c) => c.id === id);
}

/**
 * Check if there are any comments for the current file.
 */
export function hasCommentsForCurrentFile(): boolean {
  return getCommentsForCurrentFile().length > 0;
}

/**
 * Get total comment count for the current diff.
 */
export function getTotalCommentCount(): number {
  return commentsState.comments.length;
}

// =============================================================================
// Actions
// =============================================================================

/**
 * Load comments for a diff.
 */
export async function loadComments(base: string, head: string): Promise<void> {
  commentsState.loading = true;
  commentsState.diffBase = base;
  commentsState.diffHead = head;

  try {
    const review = await getReview(base, head);
    commentsState.comments = review.comments;
  } catch (e) {
    console.error('Failed to load comments:', e);
    commentsState.comments = [];
  } finally {
    commentsState.loading = false;
  }
}

/**
 * Set the current file path for filtering.
 */
export function setCurrentPath(path: string | null): void {
  commentsState.currentPath = path;
}

/**
 * Add a comment.
 */
export async function addComment(
  path: string,
  span: Span,
  content: string
): Promise<Comment | null> {
  if (!commentsState.diffBase || !commentsState.diffHead) {
    console.error('Cannot add comment: no diff selected');
    return null;
  }

  try {
    const newComment: NewComment = { path, span, content };
    const comment = await apiAddComment(commentsState.diffBase, commentsState.diffHead, newComment);
    commentsState.comments = [...commentsState.comments, comment];
    return comment;
  } catch (e) {
    console.error('Failed to add comment:', e);
    return null;
  }
}

/**
 * Update a comment's content.
 */
export async function updateComment(commentId: string, content: string): Promise<boolean> {
  try {
    await apiUpdateComment(commentId, content);
    commentsState.comments = commentsState.comments.map((c) =>
      c.id === commentId ? { ...c, content } : c
    );
    return true;
  } catch (e) {
    console.error('Failed to update comment:', e);
    return false;
  }
}

/**
 * Delete a comment.
 */
export async function deleteComment(commentId: string): Promise<boolean> {
  try {
    await apiDeleteComment(commentId);
    commentsState.comments = commentsState.comments.filter((c) => c.id !== commentId);
    return true;
  } catch (e) {
    console.error('Failed to delete comment:', e);
    return false;
  }
}

/**
 * Delete all comments for the current diff.
 */
export async function deleteAllComments(): Promise<boolean> {
  const commentIds = commentsState.comments.map((c) => c.id);

  try {
    // Delete all comments in parallel
    await Promise.all(commentIds.map((id) => apiDeleteComment(id)));
    commentsState.comments = [];
    return true;
  } catch (e) {
    console.error('Failed to delete all comments:', e);
    // Reload to get accurate state
    if (commentsState.diffBase && commentsState.diffHead) {
      await loadComments(commentsState.diffBase, commentsState.diffHead);
    }
    return false;
  }
}

/**
 * Export all comments as markdown and copy to clipboard.
 */
export async function copyCommentsToClipboard(): Promise<boolean> {
  if (!commentsState.diffBase || !commentsState.diffHead) {
    console.error('Cannot export: no diff selected');
    return false;
  }

  try {
    const markdown = await exportReviewMarkdown(commentsState.diffBase, commentsState.diffHead);
    await writeText(markdown);
    return true;
  } catch (e) {
    console.error('Failed to copy comments:', e);
    return false;
  }
}

/**
 * Clear comments state (e.g., when switching repos).
 */
export function clearComments(): void {
  commentsState.comments = [];
  commentsState.currentPath = null;
  commentsState.diffBase = null;
  commentsState.diffHead = null;
  commentsState.loading = false;
}
