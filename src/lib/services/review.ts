import { invoke } from '@tauri-apps/api/core';
import type { Review, Comment, Edit, NewComment, NewEdit } from '../types';

/**
 * Get or create a review for a diff.
 * @param base - Base ref (SHA)
 * @param head - Head ref (SHA or "WORKDIR" for working tree)
 * @param repoPath - Optional repository path
 */
export async function getReview(base: string, head: string, repoPath?: string): Promise<Review> {
  return invoke<Review>('get_review', { base, head, repoPath: repoPath ?? null });
}

/**
 * Add a comment to a review.
 */
export async function addComment(
  base: string,
  head: string,
  comment: NewComment,
  repoPath?: string
): Promise<Comment> {
  return invoke<Comment>('add_comment', { base, head, comment, repoPath: repoPath ?? null });
}

/**
 * Update a comment's content.
 */
export async function updateComment(commentId: string, content: string): Promise<void> {
  return invoke('update_comment', { commentId, content });
}

/**
 * Delete a comment from a review.
 */
export async function deleteComment(commentId: string): Promise<void> {
  return invoke('delete_comment', { commentId });
}

/**
 * Mark a file as reviewed.
 */
export async function markReviewed(base: string, head: string, path: string): Promise<void> {
  return invoke('mark_reviewed', { base, head, path });
}

/**
 * Unmark a file as reviewed.
 */
export async function unmarkReviewed(base: string, head: string, path: string): Promise<void> {
  return invoke('unmark_reviewed', { base, head, path });
}

/**
 * Record an edit made during review.
 */
export async function recordEdit(base: string, head: string, edit: NewEdit): Promise<Edit> {
  return invoke<Edit>('record_edit', { base, head, edit });
}

/**
 * Export review as markdown for clipboard.
 */
export async function exportReviewMarkdown(base: string, head: string): Promise<string> {
  return invoke<string>('export_review_markdown', { base, head });
}

/**
 * Clear a review (e.g., after commit).
 */
export async function clearReview(base: string, head: string): Promise<void> {
  return invoke('clear_review', { base, head });
}
