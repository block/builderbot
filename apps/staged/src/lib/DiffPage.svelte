<!--
  DiffPage — Full-page diff viewer with file tree sidebar.

  Adapts Mark's DiffModal layout as a full page view.
  Uses the shared @builderbot/diff-viewer components.
-->
<script lang="ts">
  import {
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    Folder,
    CirclePlus,
    CircleMinus,
    CircleArrowUp,
    MessageSquare,
    Copy,
    Check,
    Trash2,
  } from 'lucide-svelte';
  import { DiffViewer } from '@builderbot/diff-viewer/components';
  import { createDiffViewerState } from '@builderbot/diff-viewer/state';
  import {
    buildFileEntries,
    buildTree,
    compactTree,
    formatLineRange,
    truncateText,
    type FileEntry,
    type TreeNode,
  } from '@builderbot/diff-viewer/utils';
  import type { FileDiff, FileDiffSummary, Comment, Span } from '@builderbot/diff-viewer/types';
  import * as commands from './commands';
  import type { DiffSpec } from './commands';

  // ==========================================================================
  // Props
  // ==========================================================================

  interface Props {
    spec: DiffSpec;
    label: string;
    onBack: () => void;
  }

  let { spec, label, onBack }: Props = $props();

  // ==========================================================================
  // Diff viewer state (adapted for Staged's command interface)
  // ==========================================================================

  // Bridge Staged commands to the DiffCommands interface expected by the package.
  // The package expects branchId-based commands, but Staged uses DiffSpec directly.
  // We create a simple adapter.
  let files = $state<FileDiffSummary[]>([]);
  let diffCache = $state(new Map<string, FileDiff>());
  let selectedFile = $state<string | null>(null);
  let loading = $state(true);
  let loadingFile = $state<string | null>(null);
  let error = $state<string | null>(null);

  // Comments (local, non-persisted for now)
  let localComments = $state<Comment[]>([]);
  let copiedFeedback = $state(false);

  // Sidebar state
  let collapsedDirs = $state(new Set<string>());
  let sidebarCollapsed = $state(false);

  // Selection generation for ignoring stale loads
  let selectionGeneration = 0;

  // ==========================================================================
  // Derived
  // ==========================================================================

  let currentDiff = $derived(selectedFile ? (diffCache.get(selectedFile) ?? null) : null);

  let fileEntries = $derived(buildFileEntries(files, [], localComments));
  let fileTree = $derived(compactTree(buildTree(fileEntries)));

  // ==========================================================================
  // Load files on mount
  // ==========================================================================

  async function loadFiles() {
    loading = true;
    error = null;
    try {
      const response = await commands.listDiffFiles(spec);
      files = response.files;
      if (files.length > 0) {
        const firstPath = files[0].after ?? files[0].before ?? '';
        await selectFile(firstPath);
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      files = [];
    } finally {
      loading = false;
    }
  }

  async function selectFile(path: string | null) {
    const thisGeneration = ++selectionGeneration;
    selectedFile = path;

    if (path && !diffCache.has(path)) {
      loadingFile = path;
      try {
        const diff = await commands.getFileDiff(spec, path);
        if (selectionGeneration !== thisGeneration) return;
        const newCache = new Map(diffCache);
        newCache.set(path, diff);
        diffCache = newCache;
      } catch (e) {
        console.error(`Failed to load diff for ${path}:`, e);
      } finally {
        loadingFile = null;
      }
    }
  }

  // Kick off initial load
  loadFiles();

  // ==========================================================================
  // Comment handling
  // ==========================================================================

  let nextCommentId = 0;

  async function handleAddComment(path: string, span: Span, content: string): Promise<void> {
    const comment: Comment = {
      id: `local-${++nextCommentId}`,
      path,
      span,
      content,
      author: 'user',
      commentType: null,
      createdAt: Date.now(),
    };
    localComments = [...localComments, comment];
  }

  async function handleUpdateComment(commentId: string, content: string): Promise<void> {
    localComments = localComments.map((c) => (c.id === commentId ? { ...c, content } : c));
  }

  async function handleDeleteComment(commentId: string): Promise<void> {
    localComments = localComments.filter((c) => c.id !== commentId);
  }

  async function handleCopyComments() {
    if (!localComments.length) return;
    const lines: string[] = [];
    for (const c of localComments) {
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

  // ==========================================================================
  // Sidebar helpers
  // ==========================================================================

  function handleSelectFile(file: FileEntry) {
    selectFile(file.path);
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

  // ==========================================================================
  // Keyboard
  // ==========================================================================

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      onBack();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="diff-page">
  <!-- Header -->
  <div class="header" data-tauri-drag-region>
    <button class="back-btn" onclick={onBack} title="Back (Esc)">
      <ChevronLeft size={16} />
      <span>Back</span>
    </button>
    <div class="header-title" data-tauri-drag-region>
      <span class="mode-label">{label}</span>
      {#if files.length > 0}
        <span class="file-count">{files.length} file{files.length === 1 ? '' : 's'}</span>
      {/if}
    </div>
    <div class="header-spacer" data-tauri-drag-region></div>
  </div>

  <!-- Body -->
  <div class="body">
    <!-- Diff viewer -->
    <div class="diff-viewer-container">
      {#if loading}
        <div class="center-message">
          <span class="spinner"></span>
          <span>Loading diff...</span>
        </div>
      {:else if error}
        <div class="center-message error">
          <span>{error}</span>
        </div>
      {:else if files.length === 0}
        <div class="center-message">
          <span>No changes</span>
        </div>
      {:else}
        <DiffViewer
          diff={currentDiff}
          comments={localComments.filter((c) => c.path === selectedFile)}
          loading={loadingFile !== null}
          beforeLabel="before"
          afterLabel="after"
          onAddComment={handleAddComment}
          onUpdateComment={handleUpdateComment}
          onDeleteComment={handleDeleteComment}
        />
      {/if}
    </div>

    <!-- File sidebar -->
    {#if !sidebarCollapsed}
      <div class="file-sidebar">
        {#if loading}
          <div class="sidebar-loading">
            <span class="spinner small"></span>
            <span>Loading files...</span>
          </div>
        {:else if files.length > 0}
          <div class="sidebar-content">
            <!-- File tree section -->
            <div class="section-header">
              <div class="section-left"></div>
              <div class="section-divider">
                <span class="divider-label">CHANGED</span>
                <span class="count-capsule">{fileEntries.length}</span>
              </div>
              <div class="section-right"></div>
            </div>
            <ul class="tree-section">
              {#snippet treeNodes(nodes: TreeNode[], depth: number)}
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
                          {@render treeNodes(node.children, depth + 1)}
                        </ul>
                      {/if}
                    </li>
                  {:else if node.file}
                    <li class="tree-item-wrapper">
                      <button
                        class="tree-item file-item"
                        class:selected={selectedFile === node.file.path}
                        style="padding-left: {8 + depth * 12}px"
                        onclick={() => handleSelectFile(node.file!)}
                      >
                        <span class="status-icon">
                          {#if node.file.status === 'added'}
                            <CirclePlus size={16} />
                          {:else if node.file.status === 'deleted'}
                            <CircleMinus size={16} />
                          {:else}
                            <CircleArrowUp size={16} />
                          {/if}
                        </span>
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
              {@render treeNodes(fileTree, 0)}
            </ul>

            <!-- Comments section -->
            {#if localComments.length > 0}
              <div class="section-header">
                <div class="section-left"></div>
                <div class="section-divider">
                  <span class="divider-label">COMMENTS</span>
                  <span class="count-capsule">{localComments.length}</span>
                </div>
                <div class="section-right">
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
                </div>
              </div>
              <ul class="tree-section comments-section">
                {#each localComments as comment (comment.id)}
                  <li class="tree-item-wrapper">
                    <div class="comment-item-container">
                      <button
                        class="tree-item comment-item"
                        onclick={() => selectFile(comment.path)}
                      >
                        <span class="comment-icon">
                          <MessageSquare size={12} />
                        </span>
                        <span class="comment-details">
                          <span class="comment-location">
                            <span class="comment-file"
                              >{comment.path.split('/').pop() || comment.path}</span
                            >
                            <span class="comment-line">{formatLineRange(comment.span)}</span>
                          </span>
                          <span class="comment-preview">{truncateText(comment.content)}</span>
                        </span>
                      </button>
                      <button
                        class="comment-delete-btn"
                        onclick={(e) => {
                          e.stopPropagation();
                          handleDeleteComment(comment.id);
                        }}
                        title="Delete comment"
                      >
                        <Trash2 size={12} />
                      </button>
                    </div>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .diff-page {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background-color: var(--bg-chrome);
  }

  /* Header */
  .header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 16px;
    height: 40px;
    flex-shrink: 0;
    border-bottom: 1px solid var(--border-subtle);
    -webkit-app-region: drag;
  }

  .back-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    font-size: var(--size-sm);
    font-family: inherit;
    -webkit-app-region: no-drag;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .back-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .header-title {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-sm);
  }

  .mode-label {
    color: var(--text-primary);
    font-weight: 500;
  }

  .file-count {
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .header-spacer {
    flex: 1;
  }

  /* Body */
  .body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .diff-viewer-container {
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }

  .center-message {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    height: 100%;
    color: var(--text-muted);
    font-size: var(--size-md);
  }

  .center-message.error {
    color: var(--ui-danger);
  }

  /* Spinner */
  .spinner {
    display: inline-block;
    width: 16px;
    height: 16px;
    border: 2px solid var(--border-muted);
    border-top-color: var(--text-accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .spinner.small {
    width: 14px;
    height: 14px;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* File sidebar */
  .file-sidebar {
    width: 240px;
    flex-shrink: 0;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .sidebar-loading {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .sidebar-content {
    display: flex;
    flex-direction: column;
    padding: 0;
  }

  /* Section headers */
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
  }

  .section-left::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border-muted);
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

  /* Tree */
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

  .status-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .comment-indicator {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
    margin-left: auto;
    padding-left: 4px;
  }

  /* Comments section */
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
      color 0.1s;
    z-index: 1;
  }

  .comment-item-container:hover .comment-delete-btn {
    opacity: 1;
  }

  .comment-delete-btn:hover {
    color: var(--status-deleted);
  }

  .copy-btn {
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
</style>
