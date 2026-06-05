<script lang="ts">
  import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
  import Check from '@lucide/svelte/icons/check';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Folder from '@lucide/svelte/icons/folder';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import MoveRight from '@lucide/svelte/icons/move-right';
  import Plus from '@lucide/svelte/icons/plus';
  import X from '@lucide/svelte/icons/x';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { FileSearchResults } from '@builderbot/diff-viewer/components';
  import { getMatchSnippet, getTextLines, type SearchMatch } from '@builderbot/diff-viewer/utils';
  import {
    fileChangeScale,
    fileChangeTotal,
    type FileEntry,
    type TreeNode,
  } from './diffModalHelpers';
  import type { FileDiff, FileDiffSummary } from '@builderbot/diff-viewer/types';
  import type { FileSearchResult } from '@builderbot/diff-viewer/state';
  import '@builderbot/diff-viewer/components/search.css';

  interface SearchStateHandle {
    state: {
      isOpen: boolean;
      fileResults: Map<string, FileSearchResult>;
      collapsedSearchResults: Set<string>;
    };
    toggleSearchResults: (filePath: string) => void;
    areSearchResultsCollapsed: (filePath: string) => boolean;
    isCurrentResult: (files: FileDiffSummary[], filePath: string, localIndex: number) => boolean;
    getGlobalIndex: (files: FileDiffSummary[], filePath: string, localIndex: number) => number;
    setCurrentResult: (globalIndex: number) => void;
    expandFileResults: (filePath: string) => void;
    collapseFileResults: (filePath: string) => void;
    selectFirstResultInFile: (files: FileDiffSummary[], filePath: string) => boolean;
  }

  interface DiffViewerStateHandle {
    state: {
      diffCache: Map<string, FileDiff>;
    };
    getCurrentDiff: () => FileDiff | null;
    selectFile: (path: string) => Promise<void>;
  }

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
    onJumpToLine?: (lineIndex: number) => void;
    searchState?: SearchStateHandle;
    diffViewerState?: DiffViewerStateHandle;
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
    onJumpToLine,
    searchState,
    diffViewerState,
  }: Props = $props();

  // Convert FileEntry[] to FileDiffSummary[] for search functions
  const fileSummaries: FileDiffSummary[] = $derived(
    fileEntries.map((entry) => ({
      before: entry.status === 'added' ? null : entry.path,
      after: entry.status === 'deleted' ? null : entry.path,
      addedLines: entry.addedLines,
      deletedLines: entry.deletedLines,
    }))
  );

  const maxFileChangeTotal = $derived(
    fileEntries.reduce((max, entry) => Math.max(max, fileChangeTotal(entry) ?? 0), 0)
  );

  function lineCount(value: number | null | undefined): number {
    return typeof value === 'number' ? Math.max(0, value) : 0;
  }

  function changeIndicatorTitle(added: number, deleted: number): string {
    return `+${added} / -${deleted}`;
  }

  function changeIndicatorHeight(total: number): number {
    return Math.round(4 + fileChangeScale(total, maxFileChangeTotal) * 12);
  }

  function hasLineChanges(file: FileEntry): boolean {
    return (fileChangeTotal(file) ?? 0) > 0;
  }

  // Helper to get snippet for a search result
  function getSnippet(match: SearchMatch, filePath: string): string {
    if (!diffViewerState) return '';
    // Get the diff from the cache (search already loaded all diffs)
    const diff = diffViewerState.state.diffCache.get(filePath);
    if (!diff) return '';
    const afterLines = getTextLines(diff, 'after');
    return getMatchSnippet(match, afterLines);
  }

  // Handle clicking a search result
  async function handleSearchResultClick(
    filePath: string,
    match: SearchMatch,
    globalIndex: number
  ) {
    if (!searchState || !diffViewerState) return;

    // Update current result index
    searchState.setCurrentResult(globalIndex);

    // Auto-expand search results for this file
    if (searchState.areSearchResultsCollapsed(filePath)) {
      searchState.toggleSearchResults(filePath);
    }

    // Select the file and scroll to the match
    await diffViewerState.selectFile(filePath);
    // Scroll to the specific line
    if (onJumpToLine) {
      onJumpToLine(match.lineIndex);
    }
  }
</script>

{#snippet fileIcon(file: FileEntry, showReviewedSection: boolean)}
  {#if isReadonly}
    <span
      class="status-icon status-icon-static"
      class:status-icon-added={file.status === 'added'}
      class:status-icon-deleted={file.status === 'deleted'}
      class:status-icon-renamed={file.status === 'renamed' && !hasLineChanges(file)}
      class:status-icon-renamed-modified={file.status === 'renamed' && hasLineChanges(file)}
    >
      <span class="icon-default">
        {@render fileStatusVisual(file)}
      </span>
    </span>
  {:else}
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <span
            {...props}
            class="status-icon"
            class:status-icon-added={file.status === 'added'}
            class:status-icon-deleted={file.status === 'deleted'}
            class:status-icon-renamed={file.status === 'renamed' && !hasLineChanges(file)}
            class:status-icon-renamed-modified={file.status === 'renamed' && hasLineChanges(file)}
            onclick={(e) => onToggleReviewed(e, file)}
            onkeydown={(e) => e.key === 'Enter' && onToggleReviewed(e, file)}
            role="button"
            tabindex="0"
          >
            <span class="icon-default">
              {@render fileStatusVisual(file)}
            </span>
            <span class="icon-hover" class:icon-hover-unreview={showReviewedSection}>
              {#if showReviewedSection}
                <RotateCcw size={16} />
              {:else}
                <Check size={16} />
              {/if}
            </span>
          </span>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>
        {showReviewedSection ? 'Mark as needs review' : 'Mark as reviewed'}
      </Tooltip.Content>
    </Tooltip.Root>
  {/if}
{/snippet}

{#snippet fileStatusVisual(file: FileEntry)}
  {#if file.status === 'added'}
    <Plus size={16} />
  {:else if file.status === 'deleted'}
    <X size={16} />
  {:else if file.status === 'renamed'}
    <MoveRight size={16} />
  {:else}
    {@render changeIndicator(file)}
  {/if}
{/snippet}

{#snippet changeIndicator(file: FileEntry)}
  {@const total = fileChangeTotal(file)}
  {@const added = lineCount(file.addedLines)}
  {@const deleted = lineCount(file.deletedLines)}
  <Tooltip.Root disabled={!(total !== null && total > 0)}>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <span
          {...props}
          class="change-indicator"
          class:change-indicator-visible={total !== null}
          aria-hidden="true"
        >
          {#if total !== null && total > 0}
            <span class="change-indicator-fill" style:height={`${changeIndicatorHeight(total)}px`}>
              {#if added > 0}
                <span
                  class="change-segment change-segment-added"
                  style:height={`${(added / total) * 100}%`}
                ></span>
              {/if}
              {#if deleted > 0}
                <span
                  class="change-segment change-segment-deleted"
                  style:height={`${(deleted / total) * 100}%`}
                ></span>
              {/if}
            </span>
          {:else}
            <span class="change-indicator-empty"></span>
          {/if}
        </span>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Content>
      {total !== null && total > 0 ? changeIndicatorTitle(added, deleted) : ''}
    </Tooltip.Content>
  </Tooltip.Root>
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
          class:has-search-results={searchState?.state.isOpen &&
            searchState?.state.fileResults.has(node.file.path)}
          style="padding-left: {8 + depth * 12}px"
          onclick={() => onSelectFile(node.file!)}
        >
          {#if searchState?.state.isOpen}
            {#if searchState.state.fileResults.has(node.file.path)}
              <span
                class="search-chevron"
                onclick={(e) => {
                  e.stopPropagation();
                  searchState?.toggleSearchResults(node.file!.path);
                }}
                role="button"
                tabindex="0"
                onkeydown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    e.stopPropagation();
                    searchState?.toggleSearchResults(node.file!.path);
                  }
                }}
              >
                {#if searchState.areSearchResultsCollapsed(node.file.path)}
                  <ChevronRight size={14} />
                {:else}
                  <ChevronDown size={14} />
                {/if}
              </span>
            {:else}
              <span class="search-spacer"></span>
            {/if}
          {/if}
          {@render fileIcon(node.file, showReviewedSection)}
          <span class="file-name">{node.name}</span>
          {#if node.file.commentCount > 0}
            <span
              class="comment-indicator"
              class:comment-indicator-warning={node.file.commentTypes.includes('warning')}
            >
              {#if node.file.commentTypes.includes('warning')}
                <AlertTriangle size={12} />
              {:else}
                <MessageSquare size={12} />
              {/if}
            </span>
          {/if}
          {#if searchState?.state.isOpen && searchState.state.fileResults.has(node.file.path)}
            {@const resultCount =
              searchState.state.fileResults.get(node.file.path)?.matches.length ?? 0}
            <Tooltip.Root>
              <Tooltip.Trigger>
                {#snippet child({ props })}
                  <span {...props} class="search-result-count">
                    {resultCount}
                  </span>
                {/snippet}
              </Tooltip.Trigger>
              <Tooltip.Content>
                {resultCount} search result{resultCount !== 1 ? 's' : ''}
              </Tooltip.Content>
            </Tooltip.Root>
          {/if}
        </button>

        <!-- Search results (if search is active and this file has matches) -->
        {#if searchState?.state.isOpen && searchState.state.fileResults.has(node.file.path) && !searchState.areSearchResultsCollapsed(node.file.path)}
          {@const fileResult = searchState.state.fileResults.get(node.file.path)}
          {#if fileResult}
            <FileSearchResults
              {fileResult}
              filePath={node.file.path}
              {depth}
              {getSnippet}
              isCurrentResult={(fp, idx) => searchState!.isCurrentResult(fileSummaries, fp, idx)}
              onResultClick={handleSearchResultClick}
              getGlobalIndex={(fp, idx) => searchState!.getGlobalIndex(fileSummaries, fp, idx)}
              onExpandResults={(fp) => searchState!.expandFileResults(fp)}
              onCollapseResults={(fp) => searchState!.collapseFileResults(fp)}
            />
          {/if}
        {/if}
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
    box-sizing: border-box;
    width: 20px;
    height: 20px;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .status-icon-added {
    color: var(--status-added);
  }

  .status-icon-deleted {
    color: var(--status-deleted);
  }

  .status-icon-renamed {
    color: var(--status-renamed);
  }

  .status-icon-renamed-modified {
    color: var(--status-modified);
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

  .change-indicator {
    display: inline-flex;
    align-items: flex-end;
    justify-content: center;
    flex-shrink: 0;
    width: 4px;
    height: 16px;
    overflow: hidden;
    border-radius: 2px;
  }

  .change-indicator-visible {
    background: color-mix(in srgb, var(--border-muted) 35%, transparent);
  }

  .change-indicator-fill {
    display: flex;
    flex-direction: column;
    width: 100%;
    overflow: hidden;
    border-radius: inherit;
  }

  .change-indicator-empty {
    width: 100%;
    height: 6px;
    border-radius: inherit;
    background: var(--status-modified);
  }

  .change-segment {
    width: 100%;
  }

  .change-segment-added {
    background: var(--status-added);
  }

  .change-segment-deleted {
    background: var(--status-deleted);
  }

  .comment-indicator {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
    margin-left: auto;
    padding-left: 4px;
  }

  .comment-indicator.comment-indicator-warning {
    color: var(--status-modified);
  }
</style>
