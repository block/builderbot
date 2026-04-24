/**
 * Review State
 *
 * Manages lazy review creation, comments, and reviewed-path tracking.
 *
 * Key design:
 * - Opening the diff view is read-only (no DB record created).
 * - A `Review` is created on first persistent action (comment, mark reviewed)
 *   via `ensureReview`.
 * - After the review exists, all mutations go through the Tauri review commands
 *   with optimistic local updates.
 *
 * This is a factory — each DiffModal creates its own instance.
 */

import type { DiffCommands, ReviewCommands, Comment, Span, Review, FileContent } from '../types';

// =============================================================================
// Type
// =============================================================================

/** A reference file loaded into the sidebar. */
export interface ReferenceFile {
  path: string;
  content: FileContent;
}

export interface ReviewState {
  /** The review record (null until first persistent action). */
  review: Review | null;
  /** Local comments list (optimistic, kept in sync with backend). */
  comments: Comment[];
  /** Soft-deleted comments (recoverable). */
  deletedComments: Comment[];
  /** Paths marked as reviewed. */
  reviewedPaths: string[];
  /** Reference files pinned for viewing. */
  referenceFiles: ReferenceFile[];
  /** Whether the review is being created/loaded. */
  loading: boolean;
}

// =============================================================================
// Factory
// =============================================================================

/**
 * Create a reactive review state instance scoped to a branch + commit + scope.
 *
 * Does NOT create a review immediately — waits until the first persistent action.
 *
 * When `reviewId` is provided, the initial load fetches that exact review by
 * primary key instead of searching by the (branch, commit, scope) triple.
 * This is critical when multiple reviews share the same triple — without the
 * ID the lookup would return an arbitrary match.
 */
export function createReviewState(
  commands: ReviewCommands & Pick<DiffCommands, 'getFileAtRef'>,
  branchId: string,
  commitSha: string,
  scope: 'branch' | 'commit',
  reviewId?: string,
) {
  const state: ReviewState = $state({
    review: null,
    comments: [],
    deletedComments: [],
    reviewedPaths: [],
    referenceFiles: [],
    loading: false,
  });

  // =========================================================================
  // Internal: lazy review creation
  // =========================================================================

  /**
   * Ensure a review record exists. Called automatically before any mutation.
   * Returns the review ID, or null if creation failed.
   */
  async function ensureReviewExists(): Promise<string | null> {
    if (state.review) return state.review.id;

    state.loading = true;
    try {
      const review = await commands.ensureReview(branchId, commitSha, scope);
      state.review = review;
      state.comments = review.comments;
      state.reviewedPaths = review.reviewed;

      // Load reference files in background
      if (review.referenceFiles.length > 0) {
        loadReferenceFilesFromPaths(review.referenceFiles);
      }

      // Load deleted comments in background
      loadDeletedComments(review.id);

      return review.id;
    } catch (e) {
      console.error('Failed to ensure review:', e);
      return null;
    } finally {
      state.loading = false;
    }
  }

  // =========================================================================
  // Initial load: fetch existing review without creating one
  // =========================================================================

  async function loadExistingReview(): Promise<void> {
    state.loading = true;
    try {
      // When a specific reviewId was provided, fetch by primary key so we
      // always get the exact review the user clicked — even when multiple
      // reviews share the same (branch, commit, scope) triple.
      const review = reviewId
        ? await commands.getReview(reviewId)
        : await commands.findReview(branchId, commitSha, scope);
      if (review) {
        state.review = review;
        state.comments = review.comments;
        state.reviewedPaths = review.reviewed;

        if (review.referenceFiles.length > 0) {
          loadReferenceFilesFromPaths(review.referenceFiles);
        }

        // Load deleted comments in background
        loadDeletedComments(review.id);
      }
    } catch (e) {
      console.error('Failed to load existing review:', e);
    } finally {
      state.loading = false;
    }
  }

  // Fire on creation — non-blocking
  loadExistingReview();

  // =========================================================================
  // Comment actions
  // =========================================================================

  /**
   * Add a comment. Creates the review if it doesn't exist yet.
   */
  async function addComment(path: string, span: Span, content: string): Promise<void> {
    const reviewId = await ensureReviewExists();
    if (!reviewId) return;

    try {
      const comment = await commands.addComment(reviewId, path, span.start, span.end, content);
      state.comments = [...state.comments, comment];
    } catch (e) {
      console.error('Failed to add comment:', e);
    }
  }

  /**
   * Update a comment's content (optimistic).
   */
  async function updateComment(commentId: string, content: string): Promise<void> {
    // Optimistic update
    state.comments = state.comments.map((c) => (c.id === commentId ? { ...c, content } : c));

    try {
      await commands.updateComment(commentId, content);
    } catch (e) {
      console.error('Failed to update comment:', e);
      // Could reload from backend here, but for now just log.
    }
  }

  /**
   * Soft-delete a comment (optimistic — moves to deletedComments).
   */
  async function deleteComment(commentId: string): Promise<void> {
    // Optimistic: move from comments to deletedComments
    const comment = state.comments.find((c) => c.id === commentId);
    state.comments = state.comments.filter((c) => c.id !== commentId);
    if (comment) {
      state.deletedComments = [{ ...comment, deletedAt: Date.now() }, ...state.deletedComments];
    }

    try {
      await commands.deleteComment(commentId);
    } catch (e) {
      console.error('Failed to delete comment:', e);
    }
  }

  /**
   * Restore a soft-deleted comment (optimistic — moves back to comments).
   */
  async function restoreComment(commentId: string): Promise<void> {
    const comment = state.deletedComments.find((c) => c.id === commentId);
    state.deletedComments = state.deletedComments.filter((c) => c.id !== commentId);
    if (comment) {
      state.comments = [...state.comments, { ...comment, deletedAt: null }];
    }

    try {
      await commands.restoreComment(commentId);
    } catch (e) {
      console.error('Failed to restore comment:', e);
    }
  }

  /**
   * Load soft-deleted comments from backend (fire-and-forget).
   */
  async function loadDeletedComments(reviewId: string): Promise<void> {
    try {
      const deleted = await commands.getDeletedComments(reviewId);
      state.deletedComments = deleted;
    } catch (e) {
      console.error('Failed to load deleted comments:', e);
    }
  }

  // =========================================================================
  // Reviewed-path actions
  // =========================================================================

  /**
   * Mark a file as reviewed. Creates the review if it doesn't exist yet.
   */
  async function markReviewed(path: string): Promise<void> {
    const reviewId = await ensureReviewExists();
    if (!reviewId) return;

    // Optimistic
    if (!state.reviewedPaths.includes(path)) {
      state.reviewedPaths = [...state.reviewedPaths, path];
    }

    try {
      await commands.markReviewed(reviewId, path);
    } catch (e) {
      console.error('Failed to mark reviewed:', e);
      state.reviewedPaths = state.reviewedPaths.filter((p) => p !== path);
    }
  }

  /**
   * Unmark a file as reviewed (optimistic).
   */
  async function unmarkReviewed(path: string): Promise<void> {
    if (!state.review) return;

    // Optimistic
    state.reviewedPaths = state.reviewedPaths.filter((p) => p !== path);

    try {
      await commands.unmarkReviewed(state.review.id, path);
    } catch (e) {
      console.error('Failed to unmark reviewed:', e);
      state.reviewedPaths = [...state.reviewedPaths, path];
    }
  }

  /**
   * Load reference files from persisted paths (fire-and-forget).
   */
  async function loadReferenceFilesFromPaths(paths: string[]): Promise<void> {
    const results = await Promise.allSettled(
      paths.map(async (path) => {
        const file = await commands.getFileAtRef(branchId, 'HEAD', path);
        return { path: file.path, content: file.content } as ReferenceFile;
      })
    );

    const loaded: ReferenceFile[] = [];
    for (const result of results) {
      if (result.status === 'fulfilled') loaded.push(result.value);
    }
    state.referenceFiles = loaded;
  }

  /**
   * Toggle reviewed status of a file.
   */
  async function toggleReviewed(path: string): Promise<void> {
    if (isReviewed(path)) {
      await unmarkReviewed(path);
    } else {
      await markReviewed(path);
    }
  }

  /**
   * Check if a path is reviewed.
   */
  function isReviewed(path: string): boolean {
    return state.reviewedPaths.includes(path);
  }

  // =========================================================================
  // Reference file actions
  // =========================================================================

  /**
   * Add a reference file. Creates the review if it doesn't exist yet.
   * Loads file content from the repo at HEAD.
   */
  async function addReferenceFile(path: string): Promise<void> {
    // Don't add duplicates
    if (state.referenceFiles.some((f) => f.path === path)) return;

    const reviewId = await ensureReviewExists();
    if (!reviewId) return;

    try {
      const file = await commands.getFileAtRef(branchId, 'HEAD', path);
      state.referenceFiles = [...state.referenceFiles, { path: file.path, content: file.content }];
      await commands.addReferenceFile(reviewId, path);
    } catch (e) {
      console.error('Failed to add reference file:', e);
    }
  }

  /**
   * Remove a reference file (optimistic).
   */
  async function removeReferenceFile(path: string): Promise<void> {
    state.referenceFiles = state.referenceFiles.filter((f) => f.path !== path);

    if (state.review) {
      commands.removeReferenceFile(state.review.id, path).catch((e) => {
        console.error('Failed to remove reference file from DB:', e);
      });
    }
  }

  /**
   * Delete all comments (optimistic, parallel backend calls).
   */
  async function deleteAllComments(): Promise<void> {
    const now = Date.now();
    const movedComments = state.comments.map((c) => ({ ...c, deletedAt: now }));
    const ids = state.comments.map((c) => c.id);
    state.deletedComments = [...movedComments, ...state.deletedComments];
    state.comments = [];

    try {
      await Promise.all(ids.map((id) => commands.deleteComment(id)));
    } catch (e) {
      console.error('Failed to delete all comments:', e);
    }
  }

  return {
    state,
    addComment,
    updateComment,
    deleteComment,
    restoreComment,
    deleteAllComments,
    markReviewed,
    unmarkReviewed,
    toggleReviewed,
    isReviewed,
    addReferenceFile,
    removeReferenceFile,
  };
}
