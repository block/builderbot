<!--
  DiffModal.svelte — Diff viewer modal with file tree sidebar

  Opened from a branch card or timeline item. Contains:
  - DiffViewer (renders the selected file's diff)
  - File sidebar on the right with tree view, review status, reference files, comments

  Props: branchId, commitSha (optional), scope, onClose.

  State management:
  - diffViewerState: file list + on-demand diff cache
  - reviewState: lazy review creation, comments, reviewed paths, reference files
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import DiffViewer from './DiffViewer.svelte';
  import DiffCommentsSection from './DiffCommentsSection.svelte';
  import DiffFileTreeSection from './DiffFileTreeSection.svelte';
  import DiffReferenceSection from './DiffReferenceSection.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import { createDiffViewerState } from './diffViewerState.svelte';
  import { createReviewState } from './reviewState.svelte';
  import type { Comment, Span } from '../../types';
  import {
    buildFileEntries,
    buildTree,
    compactTree,
    formatLineRange,
    truncateText,
    type FileEntry,
    type TreeNode,
  } from './diffModalHelpers';

  // ==========================================================================
  // Props
  // ==========================================================================

  interface Props {
    branchId: string;
    /** Optional — for branch scope, resolved automatically. */
    commitSha?: string;
    scope?: 'branch' | 'commit';
    /** Label for the before pane header. */
    beforeLabel?: string;
    /** Label for the after pane header. */
    afterLabel?: string;
    /** When true, hides commenting, reference files, and review status. */
    readonly?: boolean;
    onClose: () => void;
  }

  let {
    branchId,
    commitSha,
    scope = 'branch',
    beforeLabel = 'base',
    afterLabel = 'head',
    readonly = false,
    onClose,
  }: Props = $props();

  // ==========================================================================
  // State
  // ==========================================================================

  // svelte-ignore state_referenced_locally
  const diffViewer = createDiffViewerState(branchId, scope, commitSha);

  type ReviewHandle = ReturnType<typeof createReviewState>;
  let reviewHandle = $state<ReviewHandle | null>(null);

  // Create review state once we have a resolved commitSha (skip in readonly mode)
  $effect(() => {
    const sha = diffViewer.state.commitSha;
    if (sha && !reviewHandle && !readonly) {
      // svelte-ignore state_referenced_locally
      reviewHandle = createReviewState(branchId, sha, scope);
    }
  });

  // Sidebar state
  let collapsedDirs = $state(new Set<string>());
  let copiedFeedback = $state(false);
  let selectedCommentId = $state<string | null>(null);
  let jumpToComment = $state<{ id: string; token: number } | null>(null);
  let commentJumpToken = 0;

  // Confirmation dialog state
  let commentToDelete = $state<string | null>(null);
  let showDeleteAllConfirm = $state(false);

  // ==========================================================================
  // Derived
  // ==========================================================================

  let currentDiff = $derived(diffViewer.getCurrentDiff());
  let currentComments = $derived(reviewHandle?.state.comments ?? []);

  let fileEntries = $derived(
    buildFileEntries(
      diffViewer.state.files,
      reviewHandle?.state.reviewedPaths ?? [],
      currentComments
    )
  );
  let needsReview = $derived(fileEntries.filter((f) => !f.isReviewed));
  let reviewed = $derived(fileEntries.filter((f) => f.isReviewed));
  let readonlyTree = $derived(compactTree(buildTree(fileEntries)));
  let needsReviewTree = $derived(compactTree(buildTree(needsReview)));
  let reviewedTree = $derived(compactTree(buildTree(reviewed)));

  // ==========================================================================
  // Sidebar interactions
  // ==========================================================================

  function selectFile(file: FileEntry) {
    selectedCommentId = null;
    diffViewer.selectFile(file.path);
  }

  async function toggleReviewed(event: MouseEvent | KeyboardEvent, file: FileEntry) {
    event.stopPropagation();
    await reviewHandle?.toggleReviewed(file.path);
  }

  function toggleDir(path: string) {
    const newSet = new Set(collapsedDirs);
    if (newSet.has(path)) newSet.delete(path);
    else newSet.add(path);
    collapsedDirs = newSet;
  }

  function isCollapsed(path: string): boolean {
    return collapsedDirs.has(path);
  }

  function getFileName(path: string): string {
    return path.split('/').pop() || path;
  }

  function handleDeleteComment(commentId: string) {
    commentToDelete = commentId;
  }

  async function confirmDeleteComment() {
    if (commentToDelete) {
      await reviewHandle?.deleteComment(commentToDelete);
      commentToDelete = null;
    }
  }

  function handleDeleteAllComments() {
    showDeleteAllConfirm = true;
  }

  async function confirmDeleteAllComments() {
    await reviewHandle?.deleteAllComments();
    showDeleteAllConfirm = false;
  }

  async function handleCopyComments() {
    if (!currentComments.length) return;
    // Build simple markdown
    const lines: string[] = [];
    for (const c of currentComments) {
      lines.push(`**${c.path}** ${formatLineRange(c.span)}`);
      lines.push(c.content);
      lines.push('');
    }
    try {
      await navigator.clipboard.writeText(lines.join('\n'));
      copiedFeedback = true;
      setTimeout(() => (copiedFeedback = false), 1500);
    } catch (e) {
      console.error('Failed to copy:', e);
    }
  }

  async function handleRemoveReferenceFile(path: string) {
    await reviewHandle?.removeReferenceFile(path);
  }

  async function handleSelectComment(comment: Comment) {
    selectedCommentId = comment.id;
    await diffViewer.selectFile(comment.path);
    commentJumpToken += 1;
    jumpToComment = { id: comment.id, token: commentJumpToken };
  }

  // ==========================================================================
  // Comment callbacks (wired to review state)
  // ==========================================================================

  async function handleAddComment(path: string, span: Span, content: string): Promise<void> {
    await reviewHandle?.addComment(path, span, content);
  }

  async function handleUpdateComment(commentId: string, content: string): Promise<void> {
    await reviewHandle?.updateComment(commentId, content);
  }

  async function handleDeleteCommentFromViewer(commentId: string): Promise<void> {
    await reviewHandle?.deleteComment(commentId);
  }

  // ==========================================================================
  // Keyboard
  // ==========================================================================

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onClose();
    }
  }

  onMount(() => {
    document.addEventListener('keydown', handleKeydown);
    return () => document.removeEventListener('keydown', handleKeydown);
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="diff-modal-backdrop" onkeydown={handleKeydown}>
  <div class="diff-modal">
    <button class="close-btn" onclick={onClose} title="Close (Esc)">
      <X size={18} />
    </button>

    <div class="modal-body">
      <!-- Diff viewer -->
      <div class="diff-viewer-container">
        <DiffViewer
          diff={currentDiff}
          comments={readonly ? [] : currentComments}
          {jumpToComment}
          loading={diffViewer.state.loadingFile !== null}
          {beforeLabel}
          {afterLabel}
          onAddComment={readonly ? undefined : handleAddComment}
          onUpdateComment={readonly ? undefined : handleUpdateComment}
          onDeleteComment={readonly ? undefined : handleDeleteCommentFromViewer}
        />
      </div>

      <!-- File sidebar (right side) -->
      <div class="file-sidebar">
        {#if diffViewer.state.loading}
          <div class="sidebar-loading">
            <Spinner size={14} />
            <span>Loading files...</span>
          </div>
        {:else if diffViewer.state.error}
          <div class="sidebar-error">
            <span>{diffViewer.state.error}</span>
          </div>
        {:else if diffViewer.state.files.length === 0}
          <div class="sidebar-empty">
            <span>No changes</span>
          </div>
        {:else}
          <div class="sidebar-content">
            <DiffFileTreeSection
              {readonly}
              {fileEntries}
              {needsReview}
              {reviewed}
              {readonlyTree}
              {needsReviewTree}
              {reviewedTree}
              selectedFile={diffViewer.state.selectedFile}
              {isCollapsed}
              onToggleDir={toggleDir}
              onSelectFile={selectFile}
              onToggleReviewed={toggleReviewed}
            />
            {#if !readonly}
              <DiffReferenceSection
                referenceFiles={reviewHandle?.state.referenceFiles ?? []}
                selectedFile={diffViewer.state.selectedFile}
                onSelectFile={(path) => {
                  selectedCommentId = null;
                  diffViewer.selectFile(path);
                }}
                onRemoveReferenceFile={handleRemoveReferenceFile}
              />

              <DiffCommentsSection
                comments={currentComments}
                {selectedCommentId}
                {copiedFeedback}
                onSelectComment={handleSelectComment}
                onCopyAll={handleCopyComments}
                onDeleteAll={handleDeleteAllComments}
                onDeleteComment={handleDeleteComment}
              />
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>

<!-- Delete comment confirmation -->
{#if commentToDelete}
  <ConfirmDialog
    title="Delete Comment"
    message="Are you sure you want to delete this comment?"
    confirmLabel="Delete"
    danger={true}
    onConfirm={confirmDeleteComment}
    onCancel={() => (commentToDelete = null)}
  />
{/if}

<!-- Delete all comments confirmation -->
{#if showDeleteAllConfirm}
  <ConfirmDialog
    title="Delete All Comments"
    message="Are you sure you want to delete all comments? This action cannot be undone."
    confirmLabel="Delete All"
    danger={true}
    onConfirm={confirmDeleteAllComments}
    onCancel={() => (showDeleteAllConfirm = false)}
  />
{/if}

<style>
  /* ========================================================================
   * Modal backdrop + frame
   * ====================================================================== */

  .diff-modal-backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background-color: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: stretch;
    justify-content: center;
    padding: 40px 0 0 0;
  }

  .diff-modal {
    display: flex;
    flex-direction: column;
    position: relative;
    width: 100%;
    height: 100%;
    background-color: var(--bg-chrome);
    border-radius: 0;
    border: none;
    box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
    overflow: hidden;
  }

  /* ========================================================================
   * Close button (floating top-right)
   * ====================================================================== */

  .close-btn {
    position: absolute;
    top: 8px;
    right: 8px;
    z-index: 10;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: var(--bg-chrome);
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  /* ========================================================================
   * Body layout
   * ====================================================================== */

  .modal-body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  /* ========================================================================
   * File sidebar
   * ====================================================================== */

  .file-sidebar {
    width: 240px;
    flex-shrink: 0;
    border-left: none;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .sidebar-content {
    display: flex;
    flex-direction: column;
    padding: 0;
  }

  .sidebar-loading,
  .sidebar-error,
  .sidebar-empty {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .sidebar-error {
    color: var(--ui-danger);
  }

  /* ========================================================================
   * Diff viewer container
   * ====================================================================== */

  .diff-viewer-container {
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }
</style>
