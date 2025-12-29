<!--
  DiffViewer.svelte - Side-by-side diff display
  
  Renders a two-pane diff view with synchronized scrolling, syntax highlighting,
  and visual connectors between corresponding changed regions. Supports panel
  minimization for new/deleted files and range-level discard operations.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X } from 'lucide-svelte';
  import type { FileDiff } from './types';
  import {
    initHighlighter,
    highlightLines,
    detectLanguage,
    prepareLanguage,
    getTheme,
    type Token,
  } from './services/highlighter';
  import { createScrollSync } from './services/scrollSync';
  import { drawConnectors } from './diffConnectors';
  import {
    getLineBoundary,
    getLanguageFromDiff,
    getFilePath,
    isBinaryDiff,
    getTextLines,
  } from './diffUtils';
  import { setupKeyboardNav } from './diffKeyboard';

  interface Props {
    diff: FileDiff | null;
    /** Head ref for the diff - "@" means working tree, enabling discard */
    diffHead?: string;
    sizeBase?: number;
    onRangeDiscard?: () => void;
  }

  let { diff, diffHead = '@', sizeBase, onRangeDiscard }: Props = $props();

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

  // Range hover state (for showing discard toolbar on changed ranges)
  let hoveredRangeIndex: number | null = $state(null);
  let rangeToolbarStyle: { top: number; left: number } | null = $state(null);

  // Discard is only available when viewing the working tree
  let canDiscard = $derived(diffHead === '@');

  // Extract lines from the diff
  let beforeLines = $derived(diff ? getTextLines(diff, 'before') : []);
  let afterLines = $derived(diff ? getTextLines(diff, 'after') : []);

  // Detect if this is a new file (no before content)
  let isNewFile = $derived(diff !== null && diff.before === null);
  // Detect if this is a deleted file (no after content)
  let isDeletedFile = $derived(diff !== null && diff.after === null);

  // Check if binary
  let isBinary = $derived(diff !== null && isBinaryDiff(diff));

  // Hide range markers (spine connectors, bounding lines, content highlights)
  // for new/deleted files since the entire file is one big change
  let showRangeMarkers = $derived(!isNewFile && !isDeletedFile);

  // Build a list of changed alignments with their indices (for hover/discard)
  let changedAlignments = $derived(
    diff?.alignments
      .map((alignment, index) => ({ alignment, index }))
      .filter(({ alignment }) => alignment.changed) ?? []
  );

  // Map line index to changed alignment index for quick lookup during hover
  let beforeLineToAlignment = $derived(() => {
    const map = new Map<number, number>();
    changedAlignments.forEach(({ alignment }, alignmentIdx) => {
      for (let i = alignment.before.start; i < alignment.before.end; i++) {
        map.set(i, alignmentIdx);
      }
    });
    return map;
  });

  let afterLineToAlignment = $derived(() => {
    const map = new Map<number, number>();
    changedAlignments.forEach(({ alignment }, alignmentIdx) => {
      for (let i = alignment.after.start; i < alignment.after.end; i++) {
        map.set(i, alignmentIdx);
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
      hoveredRangeIndex = null;
      rangeToolbarStyle = null;
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
      scrollSync.setAlignments(diff.alignments);
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
      const beforeCode = beforeLines.join('\n');
      const afterCode = afterLines.join('\n');

      beforeTokens = beforeCode ? highlightLines(beforeCode, language) : [];
      afterTokens = afterCode ? highlightLines(afterCode, language) : [];
    } else {
      // Fallback: plain text tokens
      const defaultColor = '#d4d4d4';
      beforeTokens = beforeLines.map((line) => [{ content: line, color: defaultColor }]);
      afterTokens = afterLines.map((line) => [{ content: line, color: defaultColor }]);
    }
  });

  $effect(() => {
    if (highlighterReady && diff) {
      languageReady = false;
      const path = getFilePath(diff);
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

    drawConnectors(connectorSvg, diff.alignments, beforePane.scrollTop, afterPane.scrollTop, {
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
  // Range hover handling
  // ==========================================================================

  function updateToolbarPosition() {
    if (hoveredRangeIndex === null || !afterPane || !diffViewerEl) {
      rangeToolbarStyle = null;
      return;
    }

    const alignmentData = changedAlignments[hoveredRangeIndex];
    if (!alignmentData) {
      rangeToolbarStyle = null;
      return;
    }

    // Find the first line of this alignment in the after pane
    const lineIndex = alignmentData.alignment.after.start;
    const lineEl = afterPane.querySelectorAll('.line')[lineIndex] as HTMLElement | null;

    if (!lineEl) {
      rangeToolbarStyle = null;
      return;
    }

    const lineRect = lineEl.getBoundingClientRect();
    const viewerRect = diffViewerEl.getBoundingClientRect();

    // Position toolbar above the range, aligned to left of the line
    rangeToolbarStyle = {
      top: lineRect.top - viewerRect.top,
      left: lineRect.left - viewerRect.left,
    };
  }

  function handleLineMouseEnter(pane: 'before' | 'after', lineIndex: number) {
    if (!canDiscard) return; // Don't show hover if discard not available

    const map = pane === 'before' ? beforeLineToAlignment() : afterLineToAlignment();
    const alignmentIdx = map.get(lineIndex);

    if (alignmentIdx !== undefined) {
      hoveredRangeIndex = alignmentIdx;
      requestAnimationFrame(updateToolbarPosition);
    }
  }

  function handleLineMouseLeave(event: MouseEvent) {
    // Don't clear if moving to another line in the same range or to the toolbar
    const relatedTarget = event.relatedTarget as HTMLElement | null;
    if (relatedTarget?.closest('.range-toolbar') || relatedTarget?.closest('.line')) {
      return;
    }
    hoveredRangeIndex = null;
    rangeToolbarStyle = null;
  }

  function handleToolbarMouseLeave(event: MouseEvent) {
    const relatedTarget = event.relatedTarget as HTMLElement | null;
    if (relatedTarget?.closest('.line')) {
      return;
    }
    hoveredRangeIndex = null;
    rangeToolbarStyle = null;
  }

  // ==========================================================================
  // Range actions
  // ==========================================================================

  async function handleDiscardRange() {
    if (hoveredRangeIndex === null || !canDiscard || !diff) return;

    const alignmentData = changedAlignments[hoveredRangeIndex];
    if (!alignmentData) return;

    // TODO: Implement discard via new backend API
    console.log('Discard alignment:', alignmentData.alignment);
    hoveredRangeIndex = null;
    rangeToolbarStyle = null;
    onRangeDiscard?.();
  }

  /**
   * Check if a line is within a changed alignment (for highlighting).
   */
  function isLineInChangedAlignment(side: 'before' | 'after', lineIndex: number): boolean {
    const map = side === 'before' ? beforeLineToAlignment() : afterLineToAlignment();
    return map.has(lineIndex);
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
  {:else if isBinary}
    <div class="binary-notice">
      <p>Binary file - cannot display diff</p>
    </div>
  {:else}
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
            {#each beforeLines as line, i}
              {@const boundary = showRangeMarkers
                ? getLineBoundary(diff.alignments, 'before', i)
                : { isStart: false, isEnd: false }}
              {@const isInHoveredRange =
                hoveredRangeIndex !== null && beforeLineToAlignment().get(i) === hoveredRangeIndex}
              {@const isChanged = showRangeMarkers && isLineInChangedAlignment('before', i)}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="line"
                class:range-start={boundary.isStart}
                class:range-end={boundary.isEnd}
                class:range-hovered={isInHoveredRange}
                onmouseenter={() => handleLineMouseEnter('before', i)}
                onmouseleave={handleLineMouseLeave}
              >
                <span class="line-content" class:content-changed={isChanged}>
                  {#each getBeforeTokens(i) as token}
                    <span style="color: {token.color}">{token.content}</span>
                  {/each}
                </span>
              </div>
            {/each}
            {#if beforeLines.length === 0}
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
            {#each afterLines as line, i}
              {@const boundary = showRangeMarkers
                ? getLineBoundary(diff.alignments, 'after', i)
                : { isStart: false, isEnd: false }}
              {@const isInHoveredRange =
                hoveredRangeIndex !== null && afterLineToAlignment().get(i) === hoveredRangeIndex}
              {@const isChanged = showRangeMarkers && isLineInChangedAlignment('after', i)}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="line"
                class:range-start={boundary.isStart}
                class:range-end={boundary.isEnd}
                class:range-hovered={isInHoveredRange}
                onmouseenter={() => handleLineMouseEnter('after', i)}
                onmouseleave={handleLineMouseLeave}
              >
                <span class="line-content" class:content-changed={isChanged}>
                  {#each getAfterTokens(i) as token}
                    <span style="color: {token.color}">{token.content}</span>
                  {/each}
                </span>
              </div>
            {/each}
            {#if afterLines.length === 0}
              <div class="empty-file-notice">File deleted</div>
            {/if}
          </div>
        </div>
      {/if}
    </div>

    <!-- Range action toolbar (floating, only when viewing working tree) -->
    {#if hoveredRangeIndex !== null && rangeToolbarStyle && canDiscard}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="range-toolbar"
        style="top: {rangeToolbarStyle.top}px; left: {rangeToolbarStyle.left}px;"
        onmouseleave={handleToolbarMouseLeave}
      >
        <button class="range-btn discard-btn" onclick={handleDiscardRange} title="Discard changes">
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

  .content-changed {
    background-color: var(--diff-added-overlay);
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

  .line.range-hovered {
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

  /* Range action toolbar */
  .range-toolbar {
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

  .range-btn {
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

  .range-btn:hover {
    background-color: var(--bg-tertiary);
  }

  .range-btn.discard-btn:hover {
    color: var(--status-deleted);
  }
</style>
