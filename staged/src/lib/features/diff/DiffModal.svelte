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
  import {
    X,
    Check,
    RotateCcw,
    ChevronRight,
    ChevronDown,
    Folder,
    MessageSquare,
    CirclePlus,
    CircleMinus,
    CircleArrowUp,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import DiffViewer from './DiffViewer.svelte';
  import DiffCommentsSection from './DiffCommentsSection.svelte';
  import DiffReferenceSection from './DiffReferenceSection.svelte';
  import { createDiffViewerState } from './diffViewerState.svelte';
  import { createReviewState } from './reviewState.svelte';
  import type { Span } from '../../types';
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
  let needsReviewTree = $derived(compactTree(buildTree(needsReview)));
  let reviewedTree = $derived(compactTree(buildTree(reviewed)));

  // ==========================================================================
  // Sidebar interactions
  // ==========================================================================

  function selectFile(file: FileEntry) {
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

  async function handleDeleteComment(commentId: string) {
    await reviewHandle?.deleteComment(commentId);
  }

  async function handleDeleteAllComments() {
    await reviewHandle?.deleteAllComments();
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

{#snippet fileIcon(file: FileEntry, showReviewedSection: boolean)}
  {#if readonly}
    <span class="status-icon status-icon-static">
      <span class="icon-default">
        {#if file.status === 'added'}
          <CirclePlus size={16} />
        {:else if file.status === 'deleted'}
          <CircleMinus size={16} />
        {:else}
          <CircleArrowUp size={16} />
        {/if}
      </span>
    </span>
  {:else}
    <span
      class="status-icon"
      onclick={(e) => toggleReviewed(e, file)}
      onkeydown={(e) => e.key === 'Enter' && toggleReviewed(e, file)}
      role="button"
      tabindex="0"
      title={showReviewedSection ? 'Mark as needs review' : 'Mark as reviewed'}
    >
      <span class="icon-default">
        {#if file.status === 'added'}
          <CirclePlus size={16} />
        {:else if file.status === 'deleted'}
          <CircleMinus size={16} />
        {:else}
          <CircleArrowUp size={16} />
        {/if}
      </span>
      <span class="icon-hover" class:icon-hover-unreview={showReviewedSection}>
        {#if showReviewedSection}
          <RotateCcw size={16} />
        {:else}
          <Check size={16} />
        {/if}
      </span>
    </span>
  {/if}
{/snippet}

{#snippet treeNodes(nodes: TreeNode[], depth: number, showReviewedSection: boolean)}
  {#each nodes as node (node.path)}
    {#if node.isDir}
      <li class="tree-item-wrapper">
        <button
          class="tree-item dir-item"
          style="padding-left: {8 + depth * 12}px"
          onclick={() => toggleDir(node.path)}
        >
          <span class="dir-chevron">
            {#if isCollapsed(node.path)}
              <ChevronRight size={14} />
            {:else}
              <ChevronDown size={14} />
            {/if}
          </span>
          <span class="dir-icon"><Folder size={14} /></span>
          <span class="dir-name">{node.name}</span>
        </button>
        {#if !isCollapsed(node.path)}
          <ul class="tree-children">
            {@render treeNodes(node.children, depth + 1, showReviewedSection)}
          </ul>
        {/if}
      </li>
    {:else if node.file}
      <li class="tree-item-wrapper">
        <button
          class="tree-item file-item"
          class:selected={diffViewer.state.selectedFile === node.file.path}
          style="padding-left: {8 + depth * 12}px"
          onclick={() => selectFile(node.file!)}
        >
          {@render fileIcon(node.file, showReviewedSection)}
          <span class="file-name">{node.name}</span>
          {#if node.file.commentCount > 0}
            <span class="comment-indicator"><MessageSquare size={12} /></span>
          {/if}
        </button>
      </li>
    {/if}
  {/each}
{/snippet}

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
            {#if readonly}
              <!-- Readonly mode: flat file list, no review actions -->
              <div class="section-header">
                <div class="section-left"></div>
                <div class="section-divider">
                  <span class="divider-label">CHANGED</span>
                  <span class="count-capsule">{fileEntries.length}</span>
                </div>
                <div class="section-right"></div>
              </div>
              <ul class="tree-section">
                {@render treeNodes(compactTree(buildTree(fileEntries)), 0, false)}
              </ul>
            {:else}
              <!-- Needs Review section -->
              {#if needsReview.length > 0}
                <div class="section-header">
                  <div class="section-left"></div>
                  <div class="section-divider">
                    <span class="divider-label">CHANGED</span>
                    <span class="count-capsule">{needsReview.length}</span>
                  </div>
                  <div class="section-right"></div>
                </div>
                <ul class="tree-section">
                  {@render treeNodes(needsReviewTree, 0, false)}
                </ul>
              {/if}

              <!-- Reviewed section -->
              {#if reviewed.length > 0}
                <div class="section-header">
                  <div class="section-left"></div>
                  <div class="section-divider">
                    <span class="divider-label">REVIEWED</span>
                    <span class="count-capsule">{reviewed.length}</span>
                  </div>
                  <div class="section-right"></div>
                </div>
                <ul class="tree-section reviewed-section">
                  {@render treeNodes(reviewedTree, 0, true)}
                </ul>
              {/if}

              <DiffReferenceSection
                referenceFiles={reviewHandle?.state.referenceFiles ?? []}
                selectedFile={diffViewer.state.selectedFile}
                onSelectFile={(path) => diffViewer.selectFile(path)}
                onRemoveReferenceFile={handleRemoveReferenceFile}
              />

              <DiffCommentsSection
                comments={currentComments}
                selectedFile={diffViewer.state.selectedFile}
                {copiedFeedback}
                onSelectFile={(path) => diffViewer.selectFile(path)}
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
   * Section headers (matching archive style)
   * ====================================================================== */

  .section-header {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    margin: 16px 12px 8px;
    gap: 6px;
  }

  .section-left {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 4px;
  }

  .section-left::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border-muted);
    margin-left: 4px;
  }

  .section-right {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
  }

  .section-right::before {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border-muted);
    margin-right: 4px;
  }

  .section-divider {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .divider-label {
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    text-transform: uppercase;
  }

  .count-capsule {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 16px;
    height: 14px;
    padding: 0 4px;
    background-color: var(--bg-primary);
    color: var(--text-primary);
    border-radius: 7px;
    font-size: 9px;
    font-weight: 600;
  }

  /* ========================================================================
   * Tree / file list
   * ====================================================================== */

  .tree-section {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .tree-children {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .reviewed-section {
    opacity: 0.7;
  }

  .tree-item-wrapper {
    margin: 0;
    padding: 0;
  }

  .tree-item {
    display: flex;
    align-items: center;
    width: calc(100% - 8px);
    padding: 3px 8px;
    font-size: var(--size-md);
    gap: 4px;
    cursor: pointer;
    position: relative;
    border-radius: 6px;
    margin: 0 4px;
    background: none;
    border: none;
    text-align: left;
    color: inherit;
    font-family: inherit;
    transition:
      background-color 0.1s,
      box-shadow 0.1s;
  }

  .tree-item:hover {
    background-color: var(--bg-hover);
  }

  .tree-item.selected {
    background-color: var(--bg-primary);
    box-shadow: inset 2px 0 0 var(--accent-primary);
  }

  .tree-item.selected .file-name {
    color: var(--text-primary);
    font-weight: 500;
  }

  /* Directory styling */
  .dir-item {
    color: var(--text-muted);
  }

  .dir-chevron {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 14px;
  }

  .dir-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .dir-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* File styling */
  .file-item {
    gap: 6px;
  }

  .file-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    color: var(--text-primary);
  }

  /* Status icon with hover toggle */
  .status-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    background: none;
    border: none;
    padding: 2px;
    margin: -2px;
    cursor: pointer;
    color: var(--text-muted);
    border-radius: 3px;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .status-icon:not(.status-icon-static):hover {
    background-color: var(--bg-hover);
    color: var(--status-added);
  }

  .status-icon-static {
    cursor: default;
  }

  .icon-default,
  .icon-hover {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .icon-hover {
    display: none;
  }

  .status-icon:hover .icon-default {
    display: none;
  }

  .status-icon:hover .icon-hover {
    display: flex;
  }

  .icon-hover-unreview {
    color: var(--text-muted);
  }

  /* Comment indicator */
  .comment-indicator {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
    margin-left: auto;
    padding-left: 4px;
  }

  /* ========================================================================
   * Diff viewer container
   * ====================================================================== */

  .diff-viewer-container {
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }

  :global(.spinner) {
    animation: spin 1s linear infinite;
    flex-shrink: 0;
  }
</style>
