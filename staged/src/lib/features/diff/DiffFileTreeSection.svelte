<script lang="ts">
  import {
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
  import type { FileEntry, TreeNode } from './diffModalHelpers';

  interface Props {
    readonly: boolean;
    fileEntries: FileEntry[];
    needsReview: FileEntry[];
    reviewed: FileEntry[];
    readonlyTree: TreeNode[];
    needsReviewTree: TreeNode[];
    reviewedTree: TreeNode[];
    selectedFile: string | null;
    isCollapsed: (path: string) => boolean;
    onToggleDir: (path: string) => void;
    onSelectFile: (file: FileEntry) => void;
    onToggleReviewed: (event: MouseEvent | KeyboardEvent, file: FileEntry) => void | Promise<void>;
  }

  let {
    readonly: isReadonly,
    fileEntries,
    needsReview,
    reviewed,
    readonlyTree,
    needsReviewTree,
    reviewedTree,
    selectedFile,
    isCollapsed,
    onToggleDir,
    onSelectFile,
    onToggleReviewed,
  }: Props = $props();
</script>

{#snippet fileIcon(file: FileEntry, showReviewedSection: boolean)}
  {#if isReadonly}
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
      onclick={(e) => onToggleReviewed(e, file)}
      onkeydown={(e) => e.key === 'Enter' && onToggleReviewed(e, file)}
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
          onclick={() => onToggleDir(node.path)}
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
          class:selected={selectedFile === node.file.path}
          style="padding-left: {8 + depth * 12}px"
          onclick={() => onSelectFile(node.file!)}
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

{#if isReadonly}
  <div class="section-header">
    <div class="section-left"></div>
    <div class="section-divider">
      <span class="divider-label">CHANGED</span>
      <span class="count-capsule">{fileEntries.length}</span>
    </div>
    <div class="section-right"></div>
  </div>
  <ul class="tree-section">
    {@render treeNodes(readonlyTree, 0, false)}
  </ul>
{:else}
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
{/if}

<style>
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

  .comment-indicator {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
    margin-left: auto;
    padding-left: 4px;
  }
</style>
