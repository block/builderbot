<!--
  DiffViewer.svelte - Side-by-side diff display
  
  Renders a two-pane diff view with synchronized scrolling, syntax highlighting,
  and visual connectors between corresponding changed regions. Supports panel
  minimization for new/deleted files and range-level discard operations.
  
  Alignments are loaded progressively to keep the UI responsive for large files.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X } from 'lucide-svelte';
  import type { FileDiff, Alignment } from './types';
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

  // Panel collapse state (collapsed = 10% width, not hidden)
  let beforeCollapsed = $state(false);
  let afterCollapsed = $state(false);

  // Hover state for showing collapse/expand buttons
  let beforeHovered = $state(false);
  let afterHovered = $state(false);

  // Range hover state (for showing discard toolbar on changed ranges)
  let hoveredRangeIndex: number | null = $state(null);
  let rangeToolbarStyle: { top: number; left: number } | null = $state(null);

  // ==========================================================================
  // Progressive alignment loading
  // ==========================================================================

  // How many alignments to process per batch
  const ALIGNMENT_BATCH_SIZE = 20;

  // Alignments that have been "activated" (ready to render)
  let activeAlignmentCount = $state(0);

  // The alignments to use for rendering (slice of full alignments)
  let activeAlignments = $derived.by(() => {
    if (!diff) return [];
    return diff.alignments.slice(0, activeAlignmentCount);
  });

  // Are we still loading alignments?
  let alignmentsLoading = $derived(diff !== null && activeAlignmentCount < diff.alignments.length);

  // Track the current diff to cancel loading when it changes
  let loadingForDiff: FileDiff | null = null;

  /**
   * Progressively load alignments in batches using requestIdleCallback.
   */
  function startAlignmentLoading(targetDiff: FileDiff) {
    loadingForDiff = targetDiff;
    activeAlignmentCount = 0;

    const totalAlignments = targetDiff.alignments.length;

    function loadNextBatch(deadline?: IdleDeadline) {
      // Abort if diff changed
      if (loadingForDiff !== targetDiff) return;

      // Load a batch
      const nextCount = Math.min(activeAlignmentCount + ALIGNMENT_BATCH_SIZE, totalAlignments);
      activeAlignmentCount = nextCount;

      // Schedule next batch if more to load
      if (nextCount < totalAlignments) {
        if ('requestIdleCallback' in window) {
          requestIdleCallback(loadNextBatch, { timeout: 50 });
        } else {
          setTimeout(() => loadNextBatch(), 16);
        }
      }
    }

    // Start loading
    if (totalAlignments > 0) {
      if ('requestIdleCallback' in window) {
        requestIdleCallback(loadNextBatch, { timeout: 50 });
      } else {
        setTimeout(() => loadNextBatch(), 0);
      }
    }
  }

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
  // Only includes active (loaded) alignments
  let changedAlignments = $derived(
    activeAlignments
      .map((alignment, index) => ({ alignment, index }))
      .filter(({ alignment }) => alignment.changed)
  );

  // Map line index to changed alignment index for quick lookup during hover
  // Uses activeAlignments so it updates progressively
  let beforeLineToAlignment = $derived.by(() => {
    const map = new Map<number, number>();
    changedAlignments.forEach(({ alignment }, alignmentIdx) => {
      for (let i = alignment.before.start; i < alignment.before.end; i++) {
        map.set(i, alignmentIdx);
      }
    });
    return map;
  });

  let afterLineToAlignment = $derived.by(() => {
    const map = new Map<number, number>();
    changedAlignments.forEach(({ alignment }, alignmentIdx) => {
      for (let i = alignment.after.start; i < alignment.after.end; i++) {
        map.set(i, alignmentIdx);
      }
    });
    return map;
  });

  // Auto-collapse empty panels and start alignment loading when diff changes
  $effect(() => {
    if (diff) {
      beforeCollapsed = isNewFile;
      afterCollapsed = isDeletedFile;
      // Clear hover state when diff changes
      hoveredRangeIndex = null;
      rangeToolbarStyle = null;
      // Start progressive alignment loading
      startAlignmentLoading(diff);
    } else {
      loadingForDiff = null;
      activeAlignmentCount = 0;
    }
  });

  // Sync scroll position when expanding a collapsed panel
  function expandBefore() {
    beforeCollapsed = false;
    // Sync scroll on next tick after DOM updates
    requestAnimationFrame(() => {
      if (beforePane && afterPane && diff) {
        scrollSync.onScroll('after', afterPane, beforePane);
      }
    });
  }

  function expandAfter() {
    afterCollapsed = false;
    requestAnimationFrame(() => {
      if (beforePane && afterPane && diff) {
        scrollSync.onScroll('before', beforePane, afterPane);
      }
    });
  }

  const scrollSync = createScrollSync();

  // Update scroll sync with active alignments (progressively)
  $effect(() => {
    scrollSync.setAlignments(activeAlignments);
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
    const svgRect = connectorSvg.getBoundingClientRect();
    const containerRect = beforePane.getBoundingClientRect();
    const verticalOffset = containerRect.top - svgRect.top;

    // Draw connectors for active alignments only
    drawConnectors(connectorSvg, activeAlignments, beforePane.scrollTop, afterPane.scrollTop, {
      lineHeight,
      verticalOffset,
    });
  }

  // Redraw connectors when collapse state changes (after transition)
  $effect(() => {
    const _ = [beforeCollapsed, afterCollapsed];
    // Wait for flex transition to complete
    setTimeout(redrawConnectors, 250);
  });

  // Redraw connectors when alignments load or scroll position changes
  $effect(() => {
    if (diff && connectorSvg && beforePane) {
      // Dependencies: activeAlignmentCount triggers redraw as alignments load
      const _ = [beforePane.scrollTop, activeAlignmentCount];
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

    const map = pane === 'before' ? beforeLineToAlignment : afterLineToAlignment;
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
    const map = side === 'before' ? beforeLineToAlignment : afterLineToAlignment;
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
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="diff-pane before-pane"
        class:collapsed={beforeCollapsed}
        onmouseenter={() => (beforeHovered = true)}
        onmouseleave={() => (beforeHovered = false)}
      >
        <!-- Collapse/expand button (shows on hover near top-left) -->
        {#if beforeHovered}
          <button
            class="collapse-btn"
            onclick={() => (beforeCollapsed ? expandBefore() : (beforeCollapsed = true))}
            title={beforeCollapsed ? 'Expand panel' : 'Collapse panel'}
          >
            {beforeCollapsed ? '›' : '‹'}
          </button>
        {/if}
        <div
          class="code-container"
          bind:this={beforePane}
          onscroll={handleBeforeScroll}
          style="background-color: {themeBg}"
        >
          <div class="lines-wrapper">
            {#each beforeLines as line, i}
              {@const boundary = showRangeMarkers
                ? getLineBoundary(activeAlignments, 'before', i)
                : { isStart: false, isEnd: false }}
              {@const isInHoveredRange =
                hoveredRangeIndex !== null && beforeLineToAlignment.get(i) === hoveredRangeIndex}
              {@const isChanged = showRangeMarkers && isLineInChangedAlignment('before', i)}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="line"
                class:range-start={boundary.isStart}
                class:range-end={boundary.isEnd}
                class:range-hovered={isInHoveredRange}
                class:content-changed={isChanged}
                onmouseenter={() => handleLineMouseEnter('before', i)}
                onmouseleave={handleLineMouseLeave}
              >
                <span class="line-content">
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
      </div>

      <!-- Spine between panes -->
      <div class="spine">
        {#if showRangeMarkers}
          <svg class="spine-connector" bind:this={connectorSvg}></svg>
        {:else}
          <div class="spine-placeholder"></div>
        {/if}
      </div>

      <!-- After pane -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="diff-pane after-pane"
        class:collapsed={afterCollapsed}
        onmouseenter={() => (afterHovered = true)}
        onmouseleave={() => (afterHovered = false)}
      >
        <!-- Collapse/expand button (shows on hover near top-left) -->
        {#if afterHovered}
          <button
            class="collapse-btn"
            onclick={() => (afterCollapsed ? expandAfter() : (afterCollapsed = true))}
            title={afterCollapsed ? 'Expand panel' : 'Collapse panel'}
          >
            {afterCollapsed ? '‹' : '›'}
          </button>
        {/if}
        <div
          class="code-container"
          bind:this={afterPane}
          onscroll={handleAfterScroll}
          style="background-color: {themeBg}"
        >
          <div class="lines-wrapper">
            {#each afterLines as line, i}
              {@const boundary = showRangeMarkers
                ? getLineBoundary(activeAlignments, 'after', i)
                : { isStart: false, isEnd: false }}
              {@const isInHoveredRange =
                hoveredRangeIndex !== null && afterLineToAlignment.get(i) === hoveredRangeIndex}
              {@const isChanged = showRangeMarkers && isLineInChangedAlignment('after', i)}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="line"
                class:range-start={boundary.isStart}
                class:range-end={boundary.isEnd}
                class:range-hovered={isInHoveredRange}
                class:content-changed={isChanged}
                onmouseenter={() => handleLineMouseEnter('after', i)}
                onmouseleave={handleLineMouseLeave}
              >
                <span class="line-content">
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
      </div>
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
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-width: 0;
    position: relative;
    transition: flex 0.2s ease;
  }

  /* 40/60 split: before gets 40%, after gets 60% */
  .before-pane {
    flex: 4;
  }

  .after-pane {
    flex: 6;
  }

  /* Collapsed state: 10% width (flex: 1 vs 9 for the other) */
  .before-pane.collapsed {
    flex: 1;
  }

  .after-pane.collapsed {
    flex: 1;
  }

  /* When one is collapsed, the other expands */
  .before-pane.collapsed ~ .spine ~ .after-pane {
    flex: 9;
  }

  .before-pane:not(.collapsed) ~ .spine ~ .after-pane.collapsed {
    flex: 1;
  }

  .before-pane:not(.collapsed) ~ .spine ~ .after-pane:not(.collapsed) {
    flex: 6;
  }

  /* Collapse/expand button */
  .collapse-btn {
    position: absolute;
    top: 8px;
    left: 8px;
    z-index: 10;
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    background-color: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    font-size: var(--size-lg);
    line-height: 1;
    opacity: 0.8;
    transition:
      opacity 0.15s,
      background-color 0.15s;
  }

  .collapse-btn:hover {
    opacity: 1;
    background-color: var(--bg-tertiary);
  }

  .spine {
    width: 24px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background-color: var(--bg-secondary);
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

  /* Wrapper sizes to longest line, ensuring all lines can fill its width */
  .lines-wrapper {
    display: inline-block;
    min-width: 100%;
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

  /* Changed line highlight - fills full width of lines-wrapper */
  .line.content-changed {
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
