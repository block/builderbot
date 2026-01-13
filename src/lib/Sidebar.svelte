<!--
  Sidebar.svelte - File tree with review workflow
  
  Shows files changed in the current diff (base..head) as a collapsible tree.
  Files needing review appear above the divider.
  Reviewed files appear below the divider.
  Review state comes from the shared commentsState store (single source of truth).
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    MessageSquare,
    CircleFadingArrowUp,
    CircleFadingPlus,
    CircleArrowUp,
    CirclePlus,
    CircleMinus,
    CircleX,
    Check,
    RotateCcw,
    ChevronRight,
    ChevronDown,
    Folder,
    List,
    FolderTree,
    Eye,
    X,
    Plus,
  } from 'lucide-svelte';
  import { commentsState, toggleReviewed as toggleReviewedAction } from './stores/comments.svelte';
  import { registerShortcuts } from './services/keyboard';
  import { referenceFilesState } from './stores/referenceFiles.svelte';
  import type { FileDiffSummary } from './types';

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

  interface Props {
    /** File summaries from list_diff_files */
    files: FileDiffSummary[];
    /** Whether the file list is loading */
    loading?: boolean;
    /** Called when user selects a file to view, optionally scrolling to a line */
    onFileSelect?: (path: string, scrollToLine?: number) => void;
    /** Currently selected file path */
    selectedFile?: string | null;
    /** Whether we're viewing the working tree */
    isWorkingTree?: boolean;
    /** Called when user wants to add a reference file */
    onAddReferenceFile?: () => void;
    /** Called when user wants to remove a reference file */
    onRemoveReferenceFile?: (path: string) => void;
  }

  let {
    files,
    loading = false,
    onFileSelect,
    selectedFile = null,
    isWorkingTree = true,
    onAddReferenceFile,
    onRemoveReferenceFile,
  }: Props = $props();

  let collapsedDirs = $state(new Set<string>());
  let treeView = $state(false);

  /**
   * Get the primary path for a file summary.
   */
  function getFilePath(summary: FileDiffSummary): string {
    return summary.after ?? summary.before ?? '';
  }

  /**
   * Determine file status from a FileDiffSummary.
   */
  function getFileStatus(summary: FileDiffSummary): 'added' | 'deleted' | 'modified' | 'renamed' {
    if (summary.before === null) return 'added';
    if (summary.after === null) return 'deleted';
    if (summary.before !== summary.after) return 'renamed';
    return 'modified';
  }

  /**
   * Build file list from summaries with review state.
   * Uses commentsState for both comment counts and reviewed status (reactive).
   */
  function buildFileList(
    fileSummaries: FileDiffSummary[],
    reviewedPaths: string[],
    comments: typeof commentsState.comments
  ): FileEntry[] {
    const reviewedSet = new Set(reviewedPaths);

    // Count comments per file from the shared comments state
    const commentCounts = new Map<string, number>();
    for (const comment of comments) {
      commentCounts.set(comment.path, (commentCounts.get(comment.path) || 0) + 1);
    }

    return fileSummaries.map((summary) => {
      const path = getFilePath(summary);
      return {
        path,
        status: getFileStatus(summary),
        isReviewed: reviewedSet.has(path),
        commentCount: commentCounts.get(path) || 0,
      };
    });
  }

  /**
   * Build a tree structure from a flat list of files.
   */
  function buildTree(fileEntries: FileEntry[]): TreeNode[] {
    const root: TreeNode[] = [];

    for (const file of fileEntries) {
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

    // Sort: directories first, then alphabetically
    function sortTree(nodes: TreeNode[]): TreeNode[] {
      nodes.sort((a, b) => {
        if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
        return a.name.localeCompare(b.name);
      });
      for (const node of nodes) {
        if (node.children.length > 0) {
          sortTree(node.children);
        }
      }
      return nodes;
    }

    return sortTree(root);
  }

  /**
   * Flatten single-child directory chains into combined paths.
   * e.g., src/lib/components becomes a single "src/lib/components" node if each has one child.
   */
  function compactTree(nodes: TreeNode[]): TreeNode[] {
    return nodes.map((node) => {
      if (node.isDir && node.children.length === 1 && node.children[0].isDir) {
        // Merge with single child
        const child = compactTree(node.children)[0];
        return {
          ...child,
          name: node.name + '/' + child.name,
          path: child.path,
        };
      }
      return {
        ...node,
        children: node.isDir ? compactTree(node.children) : [],
      };
    });
  }

  // Use commentsState for both comments and reviewed paths (single source of truth)
  let fileEntries = $derived(
    buildFileList(files, commentsState.reviewedPaths, commentsState.comments)
  );
  let needsReview = $derived(fileEntries.filter((f) => !f.isReviewed));
  let reviewed = $derived(fileEntries.filter((f) => f.isReviewed));

  // Build trees for each section
  let needsReviewTree = $derived(compactTree(buildTree(needsReview)));
  let reviewedTree = $derived(compactTree(buildTree(reviewed)));

  function selectFile(file: FileEntry) {
    onFileSelect?.(file.path);
  }

  async function toggleReviewed(event: MouseEvent | KeyboardEvent, file: FileEntry) {
    event.stopPropagation();
    await toggleReviewedAction(file.path);
  }

  function toggleDir(path: string) {
    const newSet = new Set(collapsedDirs);
    if (newSet.has(path)) {
      newSet.delete(path);
    } else {
      newSet.add(path);
    }
    collapsedDirs = newSet;
  }

  function isCollapsed(path: string): boolean {
    return collapsedDirs.has(path);
  }

  /**
   * Get just the filename from a full path.
   */
  function getFileName(path: string): string {
    return path.split('/').pop() || path;
  }

  /**
   * Format line range for display.
   */
  function formatLineRange(span: { start: number; end: number }): string {
    if (span.end === span.start + 1) {
      return `L${span.start + 1}`;
    }
    return `L${span.start + 1}-${span.end}`;
  }

  /**
   * Truncate text for preview.
   */
  function truncateText(text: string, maxLength = 40): string {
    const singleLine = text.replace(/\n/g, ' ').trim();
    if (singleLine.length <= maxLength) return singleLine;
    return singleLine.slice(0, maxLength).trim() + '...';
  }

  // Get flat list of file paths in display order
  function getFilePaths(): string[] {
    return files.map((f) => getFilePath(f));
  }

  // Navigate to next file
  function goToNextFile(): void {
    const paths = getFilePaths();
    if (paths.length === 0) return;

    const currentIndex = selectedFile ? paths.indexOf(selectedFile) : -1;
    const nextIndex = currentIndex < paths.length - 1 ? currentIndex + 1 : 0;
    onFileSelect?.(paths[nextIndex]);
  }

  // Navigate to previous file
  function goToPrevFile(): void {
    const paths = getFilePaths();
    if (paths.length === 0) return;

    const currentIndex = selectedFile ? paths.indexOf(selectedFile) : 0;
    const prevIndex = currentIndex > 0 ? currentIndex - 1 : paths.length - 1;
    onFileSelect?.(paths[prevIndex]);
  }

  // Register keyboard shortcuts

  onMount(() => {
    const unregister = registerShortcuts([
      {
        id: 'copy-file-path',
        keys: ['c'],
        modifiers: { meta: true, shift: true },
        description: 'Copy file path',
        category: 'files',
        handler: () => {
          if (selectedFile) {
            navigator.clipboard.writeText(selectedFile).catch((err) => {
              console.error('Failed to copy file path:', err);
            });
          }
        },
      },
      {
        id: 'next-file',
        keys: [']'],
        modifiers: { meta: true },
        description: 'Next file',
        category: 'files',
        handler: goToNextFile,
      },
      {
        id: 'prev-file',
        keys: ['['],
        modifiers: { meta: true },
        description: 'Previous file',
        category: 'files',
        handler: goToPrevFile,
      },
    ]);

    return () => unregister();
  });
</script>

{#snippet fileIcon(file: FileEntry, showReviewedSection: boolean)}
  <span
    class="status-icon"
    onclick={(e) => toggleReviewed(e, file)}
    onkeydown={(e) => e.key === 'Enter' && toggleReviewed(e, file)}
    role="button"
    tabindex="0"
    title={showReviewedSection ? 'Mark as needs review' : 'Mark as reviewed'}
  >
    <!-- Default icon (hidden on hover) -->
    <span class="icon-default">
      {#if file.status === 'added'}
        {#if isWorkingTree}
          <CircleFadingPlus size={16} />
        {:else}
          <CirclePlus size={16} />
        {/if}
      {:else if file.status === 'deleted'}
        {#if isWorkingTree}
          <CircleX size={16} />
        {:else}
          <CircleMinus size={16} />
        {/if}
      {:else if isWorkingTree}
        <CircleFadingArrowUp size={16} />
      {:else}
        <CircleArrowUp size={16} />
      {/if}
    </span>
    <!-- Hover icon -->
    <span class="icon-hover" class:icon-hover-unreview={showReviewedSection}>
      {#if showReviewedSection}
        <RotateCcw size={16} />
      {:else}
        <Check size={16} />
      {/if}
    </span>
  </span>
{/snippet}

{#snippet treeNodes(nodes: TreeNode[], depth: number, showReviewedSection: boolean)}
  {#each nodes as node (node.path)}
    {#if node.isDir}
      <!-- Directory node -->
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
          <span class="dir-icon">
            <Folder size={14} />
          </span>
          <span class="dir-name">{node.name}</span>
        </button>
        {#if !isCollapsed(node.path)}
          <ul class="tree-children">
            {@render treeNodes(node.children, depth + 1, showReviewedSection)}
          </ul>
        {/if}
      </li>
    {:else if node.file}
      <!-- File node -->
      <li class="tree-item-wrapper">
        <button
          class="tree-item file-item"
          class:selected={selectedFile === node.file.path}
          style="padding-left: {8 + depth * 12}px"
          onclick={() => selectFile(node.file!)}
        >
          {@render fileIcon(node.file, showReviewedSection)}
          <span class="file-name">{node.name}</span>
          {#if node.file.commentCount > 0}
            <span class="comment-indicator">
              <MessageSquare size={12} />
            </span>
          {/if}
        </button>
      </li>
    {/if}
  {/each}
{/snippet}

{#snippet flatFileList(fileList: FileEntry[], showReviewedSection: boolean)}
  {#each fileList as file (file.path)}
    <li class="tree-item-wrapper">
      <button
        class="tree-item file-item"
        class:selected={selectedFile === file.path}
        style="padding-left: 8px"
        onclick={() => selectFile(file)}
      >
        {@render fileIcon(file, showReviewedSection)}
        <span class="file-name">{file.path}</span>
        {#if file.commentCount > 0}
          <span class="comment-indicator">
            <MessageSquare size={12} />
          </span>
        {/if}
      </button>
    </li>
  {/each}
{/snippet}

{#snippet commentList()}
  {#each commentsState.comments as comment (comment.id)}
    <li class="tree-item-wrapper">
      <button
        class="tree-item comment-item"
        style="padding-left: 8px"
        onclick={() => onFileSelect?.(comment.path, comment.span.start)}
      >
        <span class="comment-icon">
          <MessageSquare size={12} />
        </span>
        <span class="comment-details">
          <span class="comment-location">
            <span class="comment-file">{getFileName(comment.path)}</span>
            <span class="comment-line">{formatLineRange(comment.span)}</span>
          </span>
          <span class="comment-preview">{truncateText(comment.content)}</span>
        </span>
      </button>
    </li>
  {/each}
{/snippet}

<div class="sidebar-content">
  {#if loading}
    <div class="loading-state">
      <p>Loading...</p>
    </div>
  {:else if files.length === 0}
    <div class="empty-state">
      <p>No changes</p>
      {#if isWorkingTree}
        <p class="empty-hint">Working tree is clean</p>
      {:else}
        <p class="empty-hint">No differences between refs</p>
      {/if}
    </div>
  {:else}
    <div class="file-list">
      <!-- Needs Review section -->
      {#if needsReview.length > 0}
        <div class="section-header">
          <button
            class="view-toggle"
            onclick={() => (treeView = !treeView)}
            title={treeView ? 'Switch to flat list' : 'Switch to tree view'}
          >
            {#if treeView}
              <List size={12} />
            {:else}
              <FolderTree size={12} />
            {/if}
          </button>
          <div class="section-divider">
            <span class="divider-label">CHANGED ({needsReview.length})</span>
          </div>
        </div>
        <ul class="tree-section">
          {#if treeView}
            {@render treeNodes(needsReviewTree, 0, false)}
          {:else}
            {@render flatFileList(needsReview, false)}
          {/if}
        </ul>
      {/if}

      <!-- Divider with REVIEWED label -->
      {#if reviewed.length > 0}
        <div class="section-header">
          <button
            class="view-toggle"
            onclick={() => (treeView = !treeView)}
            title={treeView ? 'Switch to flat list' : 'Switch to tree view'}
          >
            {#if treeView}
              <List size={12} />
            {:else}
              <FolderTree size={12} />
            {/if}
          </button>
          <div class="section-divider">
            <span class="divider-label">REVIEWED ({reviewed.length})</span>
          </div>
        </div>
      {/if}

      <!-- Reviewed section -->
      {#if reviewed.length > 0}
        <ul class="tree-section reviewed-section">
          {#if treeView}
            {@render treeNodes(reviewedTree, 0, true)}
          {:else}
            {@render flatFileList(reviewed, true)}
          {/if}
        </ul>
      {/if}

      <!-- Reference Files section -->
      <div class="section-header">
        <button
          class="add-file-btn"
          onclick={() => onAddReferenceFile?.()}
          title="Add reference file (Cmd+O)"
        >
          <Plus size={12} />
        </button>
        <div class="section-divider">
          <span class="divider-label">REFERENCE ({referenceFilesState.files.length})</span>
        </div>
      </div>
      {#if referenceFilesState.files.length > 0}
        <ul class="tree-section reference-section">
          {#each referenceFilesState.files as refFile (refFile.path)}
            <li class="tree-item-wrapper">
              <div
                class="tree-item file-item reference-item"
                class:selected={selectedFile === refFile.path}
                style="padding-left: 8px"
                role="button"
                tabindex="0"
                onclick={() => onFileSelect?.(refFile.path)}
                onkeydown={(e) => e.key === 'Enter' && onFileSelect?.(refFile.path)}
              >
                <span class="reference-icon">
                  <Eye size={16} />
                </span>
                <span class="file-name">{refFile.path}</span>
                <button
                  class="remove-btn"
                  onclick={(e) => {
                    e.stopPropagation();
                    onRemoveReferenceFile?.(refFile.path);
                  }}
                  onkeydown={(e) => e.key === 'Enter' && e.stopPropagation()}
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
        <div class="section-divider">
          <span class="divider-label">COMMENTS ({commentsState.comments.length})</span>
        </div>
      </div>
      {#if commentsState.comments.length > 0}
        <ul class="tree-section comments-section">
          {@render commentList()}
        </ul>
      {/if}
    </div>
  {/if}
</div>

<style>
  .sidebar-content {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .loading-state,
  .empty-state {
    padding: 20px 16px;
    text-align: center;
    color: var(--text-muted);
  }

  .empty-state p {
    margin: 0;
  }

  .empty-hint {
    font-size: var(--size-sm);
    margin-top: 4px !important;
  }

  .view-toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .view-toggle:hover {
    background-color: var(--bg-hover);
    color: var(--text-muted);
  }

  .view-toggle:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  .file-list {
    flex: 1;
    overflow-y: auto;
    padding: 8px 0;
  }

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

  /* Divider with REVIEWED label */
  .section-header {
    display: flex;
    align-items: center;
    margin: 8px 12px;
    gap: 6px;
  }

  .section-divider {
    display: flex;
    align-items: center;
    flex: 1;
    gap: 8px;
  }

  .section-divider::before,
  .section-divider::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border-subtle);
  }

  .divider-label {
    font-size: 9px;
    font-weight: 500;
    letter-spacing: 0.5px;
    color: var(--text-faint);
    text-transform: uppercase;
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
    width: 100%;
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
  }

  .tree-item:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
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

  /* Status icon as interactive span */
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

  .status-icon:hover {
    background-color: var(--bg-hover);
    color: var(--status-added);
  }

  .status-icon:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: 0;
  }

  /* Icon swap on hover */
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

  /* Unreview hover icon uses muted color instead of green */
  .icon-hover-unreview {
    color: var(--text-muted);
  }

  /* Comment indicator - must not shrink, file path will truncate instead */
  .comment-indicator {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
    margin-left: auto;
    padding-left: 4px;
  }

  /* Comments section */
  .comments-header {
    margin-top: 8px;
  }

  .comments-section {
    margin-bottom: 8px;
  }

  .comment-item {
    position: relative;
    flex-direction: column;
    align-items: flex-start !important;
    gap: 2px !important;
    padding-top: 6px !important;
    padding-bottom: 6px !important;
    padding-left: 28px !important;
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

  /* Reference files section */
  .add-file-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .add-file-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-muted);
  }

  .add-file-btn:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

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
</style>
