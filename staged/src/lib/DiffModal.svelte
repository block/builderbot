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
  import {
    X,
    Loader2,
    Check,
    RotateCcw,
    ChevronRight,
    ChevronDown,
    Folder,
    Eye,
    Trash2,
    Copy,
    MessageSquare,
    CirclePlus,
    CircleMinus,
    CircleArrowUp,
  } from 'lucide-svelte';
  import DiffViewer from './DiffViewer.svelte';
  import { createDiffViewerState, fileSummaryPath } from './stores/diffViewerState.svelte';
  import { createReviewState } from './stores/reviewState.svelte';
  import type { Span, FileDiffSummary, Comment } from './types';

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

  // ==========================================================================
  // File tree types + helpers
  // ==========================================================================

  interface FileEntry {
    path: string;
    status: 'added' | 'deleted' | 'modified' | 'renamed';
    isReviewed: boolean;
    commentCount: number;
  }

  interface TreeNode {
    name: string;
    path: string;
    isDir: boolean;
    children: TreeNode[];
    file?: FileEntry;
  }

  function fileStatus(summary: FileDiffSummary): 'added' | 'deleted' | 'modified' | 'renamed' {
    if (!summary.before) return 'added';
    if (!summary.after) return 'deleted';
    if (summary.before !== summary.after) return 'renamed';
    return 'modified';
  }

  function buildFileEntries(
    files: FileDiffSummary[],
    reviewedPaths: string[],
    comments: Comment[]
  ): FileEntry[] {
    const reviewedSet = new Set(reviewedPaths);
    const commentCounts = new Map<string, number>();
    for (const comment of comments) {
      commentCounts.set(comment.path, (commentCounts.get(comment.path) || 0) + 1);
    }

    return files.map((summary) => {
      const path = fileSummaryPath(summary);
      return {
        path,
        status: fileStatus(summary),
        isReviewed: reviewedSet.has(path),
        commentCount: commentCounts.get(path) || 0,
      };
    });
  }

  function buildTree(entries: FileEntry[]): TreeNode[] {
    const root: TreeNode[] = [];

    for (const file of entries) {
      const parts = file.path.split('/');
      let currentLevel = root;

      for (let i = 0; i < parts.length; i++) {
        const part = parts[i];
        const isLast = i === parts.length - 1;
        const pathSoFar = parts.slice(0, i + 1).join('/');

        let existing = currentLevel.find((n) => n.name === part);

        if (!existing) {
          existing = {
            name: part,
            path: pathSoFar,
            isDir: !isLast,
            children: [],
            file: isLast ? file : undefined,
          };
          currentLevel.push(existing);
        }

        if (!isLast) {
          currentLevel = existing.children;
        }
      }
    }

    function sortTree(nodes: TreeNode[]): TreeNode[] {
      nodes.sort((a, b) => {
        if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
        return a.name.localeCompare(b.name);
      });
      for (const node of nodes) {
        if (node.children.length > 0) sortTree(node.children);
      }
      return nodes;
    }

    return sortTree(root);
  }

  function compactTree(nodes: TreeNode[]): TreeNode[] {
    return nodes.map((node) => {
      if (node.isDir && node.children.length === 1 && node.children[0].isDir) {
        const child = compactTree(node.children)[0];
        return { ...child, name: node.name + '/' + child.name, path: child.path };
      }
      return { ...node, children: node.isDir ? compactTree(node.children) : [] };
    });
  }

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

  function formatLineRange(span: { start: number; end: number }): string {
    if (span.end === span.start + 1) return `L${span.start + 1}`;
    return `L${span.start + 1}-${span.end}`;
  }

  function truncateText(text: string, maxLength = 40): string {
    const singleLine = text.replace(/\n/g, ' ').trim();
    if (singleLine.length <= maxLength) return singleLine;
    return singleLine.slice(0, maxLength).trim() + '...';
  }

  async function handleDeleteComment(event: MouseEvent, commentId: string) {
    event.stopPropagation();
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

{#snippet commentList()}
  {#each currentComments as comment (comment.id)}
    <li class="tree-item-wrapper">
      <div class="comment-item-container">
        <button
          class="tree-item comment-item"
          style="padding-left: 8px"
          onclick={() => diffViewer.selectFile(comment.path)}
        >
          <span class="comment-icon"><MessageSquare size={12} /></span>
          <span class="comment-details">
            <span class="comment-location">
              <span class="comment-file">{getFileName(comment.path)}</span>
              <span class="comment-line">{formatLineRange(comment.span)}</span>
            </span>
            <span class="comment-preview">{truncateText(comment.content)}</span>
          </span>
        </button>
        <button
          class="comment-delete-btn"
          onclick={(e) => handleDeleteComment(e, comment.id)}
          title="Delete comment"
        >
          <Trash2 size={12} />
        </button>
      </div>
    </li>
  {/each}
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="diff-modal-backdrop" onkeydown={handleKeydown}>
  <div class="diff-modal">
    <!-- Header -->
    <div class="modal-header">
      <div class="header-info">
        <span class="file-count">
          {diffViewer.state.files.length} file{diffViewer.state.files.length !== 1 ? 's' : ''} changed
        </span>
        {#if currentComments.length}
          <span class="comment-count">
            · {currentComments.length} comment{currentComments.length !== 1 ? 's' : ''}
          </span>
        {/if}
      </div>
      <button class="close-btn" onclick={onClose} title="Close (Esc)">
        <X size={18} />
      </button>
    </div>

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
            <Loader2 size={14} class="spinner" />
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

              <!-- Reference Files section -->
              <div class="section-header">
                <div class="section-left"></div>
                <div class="section-divider">
                  <span class="divider-label">REFERENCE</span>
                  {#if (reviewHandle?.state.referenceFiles.length ?? 0) > 0}
                    <span class="count-capsule">{reviewHandle?.state.referenceFiles.length}</span>
                  {/if}
                </div>
                <div class="section-right">
                  <!-- TODO: wire up add reference file modal -->
                </div>
              </div>
              {#if (reviewHandle?.state.referenceFiles.length ?? 0) > 0}
                <ul class="tree-section reference-section">
                  {#each reviewHandle!.state.referenceFiles as refFile (refFile.path)}
                    <li class="tree-item-wrapper">
                      <div
                        class="tree-item file-item reference-item"
                        class:selected={diffViewer.state.selectedFile === refFile.path}
                        style="padding-left: 8px"
                        role="button"
                        tabindex="0"
                        onclick={() => diffViewer.selectFile(refFile.path)}
                        onkeydown={(e) => e.key === 'Enter' && diffViewer.selectFile(refFile.path)}
                        title={refFile.path}
                      >
                        <span class="reference-icon"><Eye size={16} /></span>
                        <span class="file-name truncate-start">{refFile.path}</span>
                        <button
                          class="remove-btn"
                          onclick={(e) => {
                            e.stopPropagation();
                            handleRemoveReferenceFile(refFile.path);
                          }}
                          title="Remove reference file"
                        >
                          <X size={12} />
                        </button>
                      </div>
                    </li>
                  {/each}
                </ul>
              {/if}

              <!-- Comments section -->
              <div class="section-header comments-header">
                <div class="section-left"></div>
                <div class="section-divider">
                  <span class="divider-label">COMMENTS</span>
                  {#if currentComments.length > 0}
                    <span class="count-capsule">{currentComments.length}</span>
                  {/if}
                </div>
                <div class="section-right">
                  {#if currentComments.length > 0}
                    <button
                      class="copy-btn"
                      class:copied={copiedFeedback}
                      onclick={handleCopyComments}
                      title="Copy all comments"
                    >
                      {#if copiedFeedback}
                        <Check size={12} />
                      {:else}
                        <Copy size={12} />
                      {/if}
                    </button>
                    <button
                      class="delete-all-btn"
                      onclick={handleDeleteAllComments}
                      title="Delete all comments"
                    >
                      <Trash2 size={12} />
                    </button>
                  {/if}
                </div>
              </div>
              {#if currentComments.length > 0}
                <ul class="tree-section comments-section">
                  {@render commentList()}
                </ul>
              {/if}
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
    align-items: center;
    justify-content: center;
    padding: 24px;
  }

  .diff-modal {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    max-width: calc(100vw - 48px);
    max-height: calc(100vh - 48px);
    background-color: var(--bg-chrome);
    border-radius: 12px;
    border: 1px solid var(--border-muted);
    box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
    overflow: hidden;
  }

  /* ========================================================================
   * Header
   * ====================================================================== */

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .header-info {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .comment-count {
    color: var(--text-faint);
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: none;
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
    border-left: 1px solid var(--border-subtle);
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
   * Reference files section
   * ====================================================================== */

  .reference-section {
    opacity: 0.85;
  }

  .reference-item {
    position: relative;
  }

  .reference-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .remove-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      background-color 0.1s,
      color 0.1s;
    margin-left: auto;
    flex-shrink: 0;
  }

  .reference-item:hover .remove-btn {
    opacity: 1;
  }

  .remove-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  /* ========================================================================
   * Comments section
   * ====================================================================== */

  .comments-section {
    margin-bottom: 8px;
  }

  .comment-item-container {
    position: relative;
    width: 100%;
  }

  .comment-item {
    position: relative;
    flex-direction: column;
    align-items: flex-start !important;
    gap: 2px !important;
    padding-top: 6px !important;
    padding-bottom: 6px !important;
    padding-left: 28px !important;
    width: 100%;
  }

  .comment-icon {
    position: absolute;
    left: 8px;
    top: 8px;
    color: var(--text-faint);
  }

  .comment-details {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    min-width: 0;
    padding-right: 32px;
  }

  .comment-location {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-xs);
  }

  .comment-file {
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .comment-line {
    flex-shrink: 0;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
  }

  .comment-preview {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .comment-delete-btn {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      color 0.1s,
      background-color 0.1s;
    z-index: 1;
  }

  .comment-item-container:hover .comment-delete-btn {
    opacity: 1;
  }

  .comment-delete-btn:hover {
    color: var(--status-deleted);
    background-color: var(--bg-primary);
  }

  /* Copy / delete-all buttons */
  .copy-btn,
  .delete-all-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .copy-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .copy-btn.copied {
    color: var(--status-added);
  }

  .delete-all-btn:hover {
    background-color: var(--bg-hover);
    color: var(--status-deleted);
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

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
