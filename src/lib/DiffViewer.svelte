<!--
  DiffViewer.svelte - Side-by-side diff display
  
  Renders a two-pane diff view with synchronized scrolling, syntax highlighting,
  and visual connectors between corresponding changed regions. Supports panel
  minimization for new/deleted files and hunk-level discard operations.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X } from 'lucide-svelte';
  import type { FileDiff } from './types';
  import type { FileCategory } from './Sidebar.svelte';
  import {
    initHighlighter,
    highlightLines,
    detectLanguage,
    prepareLanguage,
    getTheme,
    type Token,
  } from './services/highlighter';
  import { discardLines } from './services/git';
  import { createScrollSync } from './services/scrollSync';
  import { drawConnectors } from './diffConnectors';
  import { getDisplayPath, getLineBoundary, getLanguageFromDiff } from './diffUtils';
  import { setupKeyboardNav } from './diffKeyboard';

  interface Props {
    diff: FileDiff | null;
    filePath?: string | null;
    category?: FileCategory | null;
    sizeBase?: number;
    onHunkAction?: () => void;
  }

  let { diff, filePath = null, category = null, sizeBase, onHunkAction }: Props = $props();

  let beforePane: HTMLDivElement | null = $state(null);
  let afterPane: HTMLDivElement | null = $state(null);
  let connectorSvg: SVGSVGElement | null = $state(null);
  let diffViewerEl: HTMLDivElement | null = $state(null);
  let highlighterReady = $state(false);
  let languageReady = $state(false);
  let themeBg = $state('#1e1e1e');

  // Pre-computed tokens for all lines (computed once when diff/language changes)
  let beforeTokens: Token[][] = $state([]);
  let afterTokens: Token[][] = $state([]);

  // Panel minimization state
  let beforeMinimized = $state(false);
  let afterMinimized = $state(false);

  // Hunk hover state
  let hoveredHunkIndex: number | null = $state(null);
  let hunkToolbarStyle: { top: number; left: number } | null = $state(null);

  // Detect if this is a new file (no before content)
  let isNewFile = $derived(diff !== null && diff.before.lines.length === 0);
  // Detect if this is a deleted file (no after content)
  let isDeletedFile = $derived(diff !== null && diff.after.lines.length === 0);

  // Hide range markers (spine connectors, bounding lines, content highlights)
  // for new/deleted files since the entire file is one big change
  let showRangeMarkers = $derived(!isNewFile && !isDeletedFile);

  // Build a map of changed ranges (hunks) with their indices
  // Only ranges with changed: true are hunks
  let changedRanges = $derived(
    diff?.ranges.map((range, index) => ({ range, index })).filter(({ range }) => range.changed) ??
      []
  );

  // Map line index to hunk index for quick lookup
  let beforeLineToHunk = $derived(() => {
    const map = new Map<number, number>();
    changedRanges.forEach(({ range }, hunkIdx) => {
      for (let i = range.before.start; i < range.before.end; i++) {
        map.set(i, hunkIdx);
      }
    });
    return map;
  });

  let afterLineToHunk = $derived(() => {
    const map = new Map<number, number>();
    changedRanges.forEach(({ range }, hunkIdx) => {
      for (let i = range.after.start; i < range.after.end; i++) {
        map.set(i, hunkIdx);
      }
    });
    return map;
  });

  // Auto-minimize empty panels when diff changes
  $effect(() => {
    if (diff) {
      beforeMinimized = isNewFile;
      afterMinimized = isDeletedFile;
      // Clear hover state when diff changes
      hoveredHunkIndex = null;
      hunkToolbarStyle = null;
    }
  });

  // Sync scroll position when expanding a minimized panel
  function expandBefore() {
    beforeMinimized = false;
    // Sync scroll on next tick after DOM updates
    requestAnimationFrame(() => {
      if (beforePane && afterPane && diff) {
        scrollSync.onScroll('after', afterPane, beforePane);
      }
    });
  }

  function expandAfter() {
    afterMinimized = false;
    requestAnimationFrame(() => {
      if (beforePane && afterPane && diff) {
        scrollSync.onScroll('before', beforePane, afterPane);
      }
    });
  }

  const scrollSync = createScrollSync();

  $effect(() => {
    if (diff) {
      scrollSync.setRanges(diff.ranges);
    }
  });

  function handleBeforeScroll(e: Event) {
    if (!diff) return;
    const target = e.target as HTMLDivElement;
    scrollSync.onScroll('before', target, afterPane);
    redrawConnectors();
    updateToolbarPosition();
  }

  function handleAfterScroll(e: Event) {
    if (!diff) return;
    const target = e.target as HTMLDivElement;
    scrollSync.onScroll('after', target, beforePane);
    redrawConnectors();
    updateToolbarPosition();
  }

  let language = $derived(diff ? getLanguageFromDiff(diff, detectLanguage) : null);

  // Pre-compute all tokens when diff or language readiness changes
  $effect(() => {
    if (!diff) {
      beforeTokens = [];
      afterTokens = [];
      return;
    }

    if (highlighterReady && languageReady) {
      // Batch highlight all lines at once (much faster than per-line)
      const beforeCode = diff.before.lines.map((l) => l.content).join('\n');
      const afterCode = diff.after.lines.map((l) => l.content).join('\n');

      beforeTokens = beforeCode ? highlightLines(beforeCode, language) : [];
      afterTokens = afterCode ? highlightLines(afterCode, language) : [];
    } else {
      // Fallback: plain text tokens
      const defaultColor = '#d4d4d4';
      beforeTokens = diff.before.lines.map((l) => [{ content: l.content, color: defaultColor }]);
      afterTokens = diff.after.lines.map((l) => [{ content: l.content, color: defaultColor }]);
    }
  });

  $effect(() => {
    if (highlighterReady && diff) {
      languageReady = false;
      const path = diff.after.path || diff.before.path;
      if (path) {
        prepareLanguage(path).then((ready) => {
          languageReady = ready;
        });
      }
    }
  });

  // Simple lookup - tokens are pre-computed
  function getBeforeTokens(index: number): Token[] {
    return beforeTokens[index] || [{ content: '', color: '#d4d4d4' }];
  }

  function getAfterTokens(index: number): Token[] {
    return afterTokens[index] || [{ content: '', color: '#d4d4d4' }];
  }

  function redrawConnectors() {
    if (!connectorSvg || !beforePane || !afterPane || !diff) return;

    // Measure actual line height from the first line element in the DOM
    const firstLine = beforePane.querySelector('.line') as HTMLElement | null;
    const lineHeight = firstLine ? firstLine.getBoundingClientRect().height : 20;

    // Measure the structural offset between SVG top and code container top
    // This accounts for the pane-header height which scales with font size
    const svgRect = connectorSvg.getBoundingClientRect();
    const containerRect = beforePane.getBoundingClientRect();
    const verticalOffset = containerRect.top - svgRect.top;

    drawConnectors(connectorSvg, diff.ranges, beforePane.scrollTop, afterPane.scrollTop, {
      lineHeight,
      verticalOffset,
    });
  }

  // Redraw connectors when diff changes or scroll position changes
  $effect(() => {
    if (diff && connectorSvg && beforePane) {
      const _ = beforePane.scrollTop; // dependency
      redrawConnectors();
    }
  });

  // Redraw connectors when font size changes
  $effect(() => {
    if (sizeBase && diff && connectorSvg && beforePane) {
      // Wait for DOM to update with new font size
      requestAnimationFrame(() => {
        redrawConnectors();
      });
    }
  });

  // ==========================================================================
  // Hunk hover handling
  // ==========================================================================

  function updateToolbarPosition() {
    if (hoveredHunkIndex === null || !afterPane || !diffViewerEl) {
      hunkToolbarStyle = null;
      return;
    }

    const hunkData = changedRanges[hoveredHunkIndex];
    if (!hunkData) {
      hunkToolbarStyle = null;
      return;
    }

    // Find the first line of this hunk in the after pane
    const lineIndex = hunkData.range.after.start;
    const lineEl = afterPane.querySelectorAll('.line')[lineIndex] as HTMLElement | null;

    if (!lineEl) {
      hunkToolbarStyle = null;
      return;
    }

    const lineRect = lineEl.getBoundingClientRect();
    const viewerRect = diffViewerEl.getBoundingClientRect();

    // Position toolbar above the hunk, aligned to left of the line
    hunkToolbarStyle = {
      top: lineRect.top - viewerRect.top,
      left: lineRect.left - viewerRect.left,
    };
  }

  function handleLineMouseEnter(pane: 'before' | 'after', lineIndex: number) {
    const map = pane === 'before' ? beforeLineToHunk() : afterLineToHunk();
    const hunkIdx = map.get(lineIndex);

    if (hunkIdx !== undefined) {
      hoveredHunkIndex = hunkIdx;
      requestAnimationFrame(updateToolbarPosition);
    }
  }

  function handleLineMouseLeave(event: MouseEvent) {
    // Don't clear if moving to another line in the same hunk or to the toolbar
    const relatedTarget = event.relatedTarget as HTMLElement | null;
    if (relatedTarget?.closest('.hunk-toolbar') || relatedTarget?.closest('.line')) {
      return;
    }
    hoveredHunkIndex = null;
    hunkToolbarStyle = null;
  }

  function handleToolbarMouseLeave(event: MouseEvent) {
    const relatedTarget = event.relatedTarget as HTMLElement | null;
    if (relatedTarget?.closest('.line')) {
      // Moving back to a line - check if it's in the same hunk
      return;
    }
    hoveredHunkIndex = null;
    hunkToolbarStyle = null;
  }

  // ==========================================================================
  // Hunk actions
  // ==========================================================================

  async function handleDiscardRange() {
    if (hoveredHunkIndex === null || !filePath || !category) return;

    const hunkData = changedRanges[hoveredHunkIndex];
    if (!hunkData?.range.source_lines) {
      console.error('No source_lines data for range');
      return;
    }

    try {
      await discardLines(filePath, hunkData.range.source_lines, category === 'staged');
      hoveredHunkIndex = null;
      hunkToolbarStyle = null;
      onHunkAction?.();
    } catch (e) {
      console.error('Failed to discard range:', e);
    }
  }

  onMount(() => {
    initHighlighter('github-dark').then(() => {
      const theme = getTheme();
      if (theme) themeBg = theme.bg;
      highlighterReady = true;
    });

    return setupKeyboardNav({
      getScrollTarget: () => afterPane,
    });
  });
</script>

<div class="diff-viewer" bind:this={diffViewerEl}>
  {#if diff === null}
    <div class="empty-state">
      <p>Select a file to view changes</p>
    </div>
  {:else if diff.is_binary}
    <div class="diff-header">
      <span class="file-path">{getDisplayPath(diff)}</span>
    </div>
    <div class="binary-notice">
      <p>Binary file - cannot display diff</p>
    </div>
  {:else}
    <div class="diff-header">
      <span class="file-path">{getDisplayPath(diff)}</span>
    </div>

    <div class="diff-content">
      <!-- Before pane -->
      {#if beforeMinimized}
        <button class="minimized-pane" onclick={expandBefore} title="Expand before panel">
          <span class="minimized-label">Before</span>
          <span class="expand-icon">›</span>
        </button>
      {:else}
        <div class="diff-pane">
          <div class="pane-header">
            <span>Before</span>
            <button
              class="minimize-btn"
              onclick={() => (beforeMinimized = true)}
              title="Minimize panel"
            >
              ‹
            </button>
          </div>
          <div
            class="code-container"
            bind:this={beforePane}
            onscroll={handleBeforeScroll}
            style="background-color: {themeBg}"
          >
            {#each diff.before.lines as line, i}
              {@const boundary = showRangeMarkers
                ? getLineBoundary(diff.ranges, 'before', i)
                : { isStart: false, isEnd: false }}
              {@const isInHoveredHunk =
                hoveredHunkIndex !== null && beforeLineToHunk().get(i) === hoveredHunkIndex}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="line"
                class:line-removed={line.line_type === 'removed'}
                class:range-start={boundary.isStart}
                class:range-end={boundary.isEnd}
                class:hunk-hovered={isInHoveredHunk}
                onmouseenter={() => handleLineMouseEnter('before', i)}
                onmouseleave={handleLineMouseLeave}
              >
                <span
                  class="line-content"
                  class:content-removed={showRangeMarkers && line.line_type === 'removed'}
                >
                  {#each getBeforeTokens(i) as token}
                    <span style="color: {token.color}">{token.content}</span>
                  {/each}
                </span>
              </div>
            {/each}
            {#if diff.before.lines.length === 0}
              <div class="empty-file-notice">New file</div>
            {/if}
          </div>
        </div>
      {/if}

      <!-- Spine between panes (always visible for consistency) -->
      <div class="spine">
        <div class="spine-header"></div>
        <!-- Range connectors only drawn when showRangeMarkers is true -->
        {#if showRangeMarkers}
          <svg class="spine-connector" bind:this={connectorSvg}></svg>
        {:else}
          <div class="spine-placeholder"></div>
        {/if}
      </div>

      <!-- After pane -->
      {#if afterMinimized}
        <button class="minimized-pane" onclick={expandAfter} title="Expand after panel">
          <span class="expand-icon">‹</span>
          <span class="minimized-label">After</span>
        </button>
      {:else}
        <div class="diff-pane">
          <div class="pane-header">
            <span>After</span>
            <button
              class="minimize-btn"
              onclick={() => (afterMinimized = true)}
              title="Minimize panel"
            >
              ›
            </button>
          </div>
          <div
            class="code-container"
            bind:this={afterPane}
            onscroll={handleAfterScroll}
            style="background-color: {themeBg}"
          >
            {#each diff.after.lines as line, i}
              {@const boundary = showRangeMarkers
                ? getLineBoundary(diff.ranges, 'after', i)
                : { isStart: false, isEnd: false }}
              {@const isInHoveredHunk =
                hoveredHunkIndex !== null && afterLineToHunk().get(i) === hoveredHunkIndex}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="line"
                class:line-added={line.line_type === 'added'}
                class:range-start={boundary.isStart}
                class:range-end={boundary.isEnd}
                class:hunk-hovered={isInHoveredHunk}
                onmouseenter={() => handleLineMouseEnter('after', i)}
                onmouseleave={handleLineMouseLeave}
              >
                <span
                  class="line-content"
                  class:content-added={showRangeMarkers && line.line_type === 'added'}
                >
                  {#each getAfterTokens(i) as token}
                    <span style="color: {token.color}">{token.content}</span>
                  {/each}
                </span>
              </div>
            {/each}
            {#if diff.after.lines.length === 0}
              <div class="empty-file-notice">File deleted</div>
            {/if}
          </div>
        </div>
      {/if}
    </div>

    <!-- Hunk action toolbar (floating) -->
    {#if hoveredHunkIndex !== null && hunkToolbarStyle && filePath && category}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="hunk-toolbar"
        style="top: {hunkToolbarStyle.top}px; left: {hunkToolbarStyle.left}px;"
        onmouseleave={handleToolbarMouseLeave}
      >
        <button class="hunk-btn discard-btn" onclick={handleDiscardRange} title="Discard changes">
          <X size={12} />
        </button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .diff-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    position: relative;
  }

  .diff-content {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  .diff-pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-width: 0;
  }

  .diff-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 16px;
    background-color: var(--diff-header-bg);
    border-bottom: 1px solid var(--border-primary);
  }

  .file-path {
    font-family: monospace;
    font-size: var(--size-md);
    color: var(--status-modified);
  }

  .pane-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 12px;
    font-size: var(--size-xs);
    text-transform: uppercase;
    color: var(--text-muted);
    background-color: var(--diff-header-bg);
    border-bottom: 1px solid var(--border-primary);
  }

  .minimize-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0 4px;
    font-size: var(--size-lg);
    line-height: 1;
    opacity: 0.6;
    transition: opacity 0.15s;
  }

  .minimize-btn:hover {
    opacity: 1;
  }

  .minimized-pane {
    width: 28px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    background-color: var(--bg-secondary);
    border: none;
    border-left: 1px solid var(--border-primary);
    border-right: 1px solid var(--border-primary);
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .minimized-pane:first-child {
    border-left: none;
  }

  .minimized-pane:last-child {
    border-right: none;
  }

  .minimized-pane:hover {
    background-color: var(--bg-tertiary);
  }

  .minimized-label {
    writing-mode: vertical-rl;
    text-orientation: mixed;
    font-size: var(--size-xs);
    text-transform: uppercase;
    color: var(--text-muted);
    letter-spacing: 0.5px;
  }

  .expand-icon {
    color: var(--text-muted);
    font-size: var(--size-lg);
  }

  .spine {
    width: 24px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background-color: var(--bg-secondary);
  }

  .spine-header {
    height: 29px;
    background-color: var(--diff-header-bg);
    border-bottom: 1px solid var(--border-primary);
  }

  .spine-connector,
  .spine-placeholder {
    flex: 1;
    width: 100%;
    overflow: visible;
  }

  .code-container {
    flex: 1;
    overflow: auto;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-md);
    line-height: 1.5;
    min-width: 0;
    scrollbar-width: none;
    -ms-overflow-style: none;
  }

  .code-container::-webkit-scrollbar {
    display: none;
  }

  .line {
    display: flex;
    min-height: calc(var(--size-md) * 1.5);
    position: relative;
  }

  .line-content {
    flex: 1;
    padding: 0 12px;
    white-space: pre;
  }

  .content-added {
    background-color: var(--diff-added-overlay);
  }

  .content-removed {
    background-color: var(--diff-removed-overlay);
  }

  .line.range-start::before {
    content: '';
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 1px;
    background-color: var(--diff-line-number);
    opacity: 0.7;
  }

  .line.range-end::after {
    content: '';
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    height: 1px;
    background-color: var(--diff-line-number);
    opacity: 0.7;
  }

  .line.hunk-hovered {
    background-color: var(--bg-tertiary);
  }

  .empty-state,
  .binary-notice {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: var(--size-lg);
  }

  .empty-file-notice {
    padding: 20px;
    color: var(--text-muted);
    font-style: italic;
  }

  /* Hunk action toolbar */
  .hunk-toolbar {
    position: absolute;
    display: flex;
    gap: 1px;
    transform: translateY(-100%);
    z-index: 100;
    background-color: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    border-bottom: none;
    border-radius: 4px 4px 0 0;
  }

  .hunk-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px 8px;
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: 3px 3px 0 0;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .hunk-btn:hover {
    background-color: var(--bg-tertiary);
  }

  .hunk-btn.discard-btn:hover {
    color: var(--status-deleted);
  }
</style>
