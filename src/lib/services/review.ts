import { invoke } from '@tauri-apps/api/core';
import type { DiffSpec, Review, Comment, Edit, NewComment, NewEdit } from '../types';

/**
 * Get or create a review for a diff.
 */
export async function getReview(spec: DiffSpec, repoPath?: string): Promise<Review> {
  return invoke<Review>('get_review', { repoPath: repoPath ?? null, spec });
}

/**
 * Add a comment to a review.
 */
export async function addComment(
  spec: DiffSpec,
  comment: NewComment,
  repoPath?: string
): Promise<Comment> {
  return invoke<Comment>('add_comment', { repoPath: repoPath ?? null, spec, comment });
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
export async function markReviewed(spec: DiffSpec, path: string, repoPath?: string): Promise<void> {
  return invoke('mark_reviewed', { repoPath: repoPath ?? null, spec, path });
}

/**
 * Unmark a file as reviewed.
 */
export async function unmarkReviewed(
  spec: DiffSpec,
  path: string,
  repoPath?: string
): Promise<void> {
  return invoke('unmark_reviewed', { repoPath: repoPath ?? null, spec, path });
}

/**
 * Record an edit made during review.
 */
export async function recordEdit(spec: DiffSpec, edit: NewEdit, repoPath?: string): Promise<Edit> {
  return invoke<Edit>('record_edit', { repoPath: repoPath ?? null, spec, edit });
}

/**
 * Export review as markdown for clipboard.
 */
export async function exportReviewMarkdown(spec: DiffSpec, repoPath?: string): Promise<string> {
  return invoke<string>('export_review_markdown', { repoPath: repoPath ?? null, spec });
}

/**
 * Clear a review (e.g., after commit).
 */
export async function clearReview(spec: DiffSpec, repoPath?: string): Promise<void> {
  return invoke('clear_review', { repoPath: repoPath ?? null, spec });
}

/**
 * Add a reference file path to a review.
 */
export async function addReferenceFilePath(
  spec: DiffSpec,
  path: string,
  repoPath?: string
): Promise<void> {
  return invoke('add_reference_file', { repoPath: repoPath ?? null, spec, path });
}

/**
 * Remove a reference file path from a review.
 */
export async function removeReferenceFilePath(
  spec: DiffSpec,
  path: string,
  repoPath?: string
): Promise<void> {
  return invoke('remove_reference_file', { repoPath: repoPath ?? null, spec, path });
}
