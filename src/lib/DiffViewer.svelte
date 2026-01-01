<!--
  DiffViewer.svelte - Side-by-side diff display
  
  Renders a two-pane diff view with synchronized scrolling, syntax highlighting,
  and visual connectors between corresponding changed regions. Supports panel
  minimization for new/deleted files and range-level discard operations.
  
  Alignments are loaded progressively to keep the UI responsive for large files.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X, GitBranch } from 'lucide-svelte';
  import type { FileDiff, Alignment } from './types';
  import {
    initHighlighter,
    highlightLines,
    detectLanguage,
    prepareLanguage,
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
  import { WORKDIR } from './stores/diffSelection.svelte';

  /** Number of alignments to process per batch during progressive loading */
  const ALIGNMENT_BATCH_SIZE = 20;

  /** Duration (ms) for panel flex transitions - used to schedule connector redraws */
  const PANEL_TRANSITION_MS = 250;

  /**
   * Set up space key handling for zoom modifier.
   * Space held = 90/10 split instead of 60/40 (like zoom key in photo editors).
   */
  function setupSpaceKeyHandler(onSpaceChange: (held: boolean) => void): () => void {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.code === 'Space' && !e.repeat) {
        const target = e.target as HTMLElement;
        // Allow space in text inputs
        if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') {
          return;
        }
        // Blur focusable elements to capture space
        if (document.activeElement instanceof HTMLElement) {
          document.activeElement.blur();
        }
        e.preventDefault();
        e.stopPropagation();
        onSpaceChange(true);
      }
    }

    function handleKeyUp(e: KeyboardEvent) {
      if (e.code === 'Space') {
        onSpaceChange(false);
      }
    }

    window.addEventListener('keydown', handleKeyDown, { capture: true });
    window.addEventListener('keyup', handleKeyUp, { capture: true });

    return () => {
      window.removeEventListener('keydown', handleKeyDown, { capture: true });
      window.removeEventListener('keyup', handleKeyUp, { capture: true });
    };
  }

  interface Props {
    diff: FileDiff | null;
    /** Base ref for the diff (before side) */
    diffBase?: string;
    /** Head ref for the diff - WORKDIR means working tree, enabling discard */
    diffHead?: string;
    sizeBase?: number;
    /** Bumped when syntax theme changes to trigger re-highlight */
    syntaxThemeVersion?: number;
    onRangeDiscard?: () => void;
  }

  let {
    diff,
    diffBase = 'HEAD',
    diffHead = WORKDIR,
    sizeBase,
    syntaxThemeVersion = 0,
    onRangeDiscard,
  }: Props = $props();

  let beforePane: HTMLDivElement | null = $state(null);
  let afterPane: HTMLDivElement | null = $state(null);
  let connectorSvg: SVGSVGElement | null = $state(null);
  let diffViewerEl: HTMLDivElement | null = $state(null);
  let highlighterReady = $state(false);
  let languageReady = $state(false);

  // Pre-computed tokens for all lines (computed once when diff/language changes)
  let beforeTokens: Token[][] = $state([]);
  let afterTokens: Token[][] = $state([]);

  // Panel collapse state (collapsed = 10% width, not hidden)
  let beforeCollapsed = $state(false);
  let afterCollapsed = $state(false);

  // Panel hover state for dynamic sizing
  let beforeHovered = $state(false);
  let afterHovered = $state(false);

  // Space key held = 90/10 split instead of 60/40 (like zoom key in photo editors)
  let spaceHeld = $state(false);

  // Range hover state (for showing discard toolbar on changed ranges)
  let hoveredRangeIndex: number | null = $state(null);
  let rangeToolbarStyle: { top: number; left: number } | null = $state(null);

  // ==========================================================================
  // Progressive alignment loading
  // ==========================================================================

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
  let canDiscard = $derived(diffHead === WORKDIR);

  // Extract lines from the diff
  let beforeLines = $derived(diff ? getTextLines(diff, 'before') : []);
  let afterLines = $derived(diff ? getTextLines(diff, 'after') : []);

  // Detect if this is a new file (no before content)
  let isNewFile = $derived(diff !== null && diff.before === null);
  // Detect if this is a deleted file (no after content)
  let isDeletedFile = $derived(diff !== null && diff.after === null);

  // File paths for headers
  let beforePath = $derived(diff?.before?.path ?? null);
  let afterPath = $derived(diff?.after?.path ?? null);

  // Helper to get just the filename from a path
  function getFileName(path: string | null): string {
    if (!path) return '';
    return path.split('/').pop() || path;
  }

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

  // Pre-compute all tokens when diff, language, or syntax theme changes
  $effect(() => {
    // Include syntaxThemeVersion in dependencies to re-highlight on theme change
    const _version = syntaxThemeVersion;

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
      // Fallback: plain text tokens using 'inherit' to use CSS variable color
      beforeTokens = beforeLines.map((line) => [{ content: line, color: 'inherit' }]);
      afterTokens = afterLines.map((line) => [{ content: line, color: 'inherit' }]);
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
    return beforeTokens[index] || [{ content: '', color: 'inherit' }];
  }

  function getAfterTokens(index: number): Token[] {
    return afterTokens[index] || [{ content: '', color: 'inherit' }];
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
      hoveredIndex: hoveredRangeIndex,
    });
  }

  // Redraw connectors and update toolbar when panel size state changes (after transition)
  $effect(() => {
    const _ = [beforeCollapsed, afterCollapsed, beforeHovered, afterHovered, spaceHeld];
    // Wait for flex transition to complete
    setTimeout(() => {
      redrawConnectors();
      updateToolbarPosition();
    }, PANEL_TRANSITION_MS);
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

  // Redraw connectors when hover state changes
  $effect(() => {
    // Dependency on hoveredRangeIndex
    const _ = hoveredRangeIndex;
    if (diff && connectorSvg && beforePane) {
      redrawConnectors();
    }
  });

  // Redraw connectors when syntax theme changes (CSS variables update)
  $effect(() => {
    const _version = syntaxThemeVersion;
    if (diff && connectorSvg && beforePane) {
      // Small delay to ensure CSS variables have been applied
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
    // Highlighter is initialized by App with the saved theme.
    // We just wait for it to be ready (initHighlighter is idempotent).
    initHighlighter().then(() => {
      highlighterReady = true;
    });

    // Space key = zoom modifier (hold to expand hovered panel to 90%)
    const cleanupSpaceKey = setupSpaceKeyHandler((held) => {
      spaceHeld = held;
    });

    // Arrow keys and Ctrl+N/P for scrolling
    const cleanupKeyboardNav = setupKeyboardNav({
      getScrollTarget: () => afterPane,
    });

    return () => {
      cleanupSpaceKey();
      cleanupKeyboardNav?.();
    };
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
        class:focused={beforeHovered && !beforeCollapsed}
        class:zoomed={beforeHovered && !beforeCollapsed && spaceHeld}
        onmouseenter={() => (beforeHovered = true)}
        onmouseleave={() => (beforeHovered = false)}
      >
        <div class="pane-header">
          <span class="pane-ref">
            <GitBranch size={12} />
            {diffBase}
          </span>
          <span class="pane-path" title={beforePath}>{beforePath ?? 'No file'}</span>
        </div>
        <div class="code-container" bind:this={beforePane} onscroll={handleBeforeScroll}>
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
              <div class="empty-pane-notice">
                <span class="empty-pane-label">No previous version</span>
              </div>
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
        class:focused={afterHovered && !afterCollapsed}
        class:zoomed={afterHovered && !afterCollapsed && spaceHeld}
        onmouseenter={() => (afterHovered = true)}
        onmouseleave={() => (afterHovered = false)}
      >
        <div class="pane-header">
          <span class="pane-ref">
            <GitBranch size={12} />
            {diffHead === WORKDIR ? 'Working Tree' : diffHead}
          </span>
          <span class="pane-path" title={afterPath}>{afterPath ?? 'No file'}</span>
        </div>
        <div class="code-container" bind:this={afterPane} onscroll={handleAfterScroll}>
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
              <div class="empty-pane-notice">
                <span class="empty-pane-label">File deleted</span>
              </div>
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
    /* Small left margin for before pane */
    padding-left: 8px;
  }

  .diff-pane {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-width: 0;
    position: relative;
    transition: flex 0.2s ease;
    /* Island styling */
    border-radius: 12px;
    background-color: var(--bg-primary);
  }

  /* Pane header - ref and file path */
  .pane-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    flex-shrink: 0;
    border-bottom: 1px solid var(--border-subtle);
  }

  .pane-ref {
    display: flex;
    align-items: center;
    gap: 4px;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-xs);
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .pane-path {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-sm);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Default: 40/60 split (before gets 40%, after gets 60%) */
  .before-pane {
    flex: 4;
  }

  .after-pane {
    flex: 6;
  }

  /* Focused (hovered): 60/40 split - hovered pane expands */
  .before-pane.focused:not(.zoomed) {
    flex: 6;
  }

  .before-pane.focused:not(.zoomed) ~ .spine ~ .after-pane:not(.focused) {
    flex: 4;
  }

  .after-pane.focused:not(.zoomed) {
    flex: 6;
  }

  .before-pane:not(.focused) ~ .spine ~ .after-pane.focused:not(.zoomed) {
    flex: 6;
  }

  /* Zoomed (space held): 90/10 split - zoomed pane dominates */
  .before-pane.zoomed {
    flex: 9;
  }

  .before-pane.zoomed ~ .spine ~ .after-pane {
    flex: 1;
  }

  .after-pane.zoomed {
    flex: 9;
  }

  .before-pane:not(.zoomed) ~ .spine ~ .after-pane.zoomed {
    flex: 9;
  }

  /* When after is zoomed, before shrinks */
  .before-pane:has(~ .spine ~ .after-pane.zoomed) {
    flex: 1;
  }

  /* Collapsed state: fixed 10% width, ignores hover/focus states */
  .before-pane.collapsed {
    flex: 1 !important;
  }

  .after-pane.collapsed {
    flex: 1 !important;
  }

  /* When one is collapsed, the other expands to 90% (also fixed) */
  .before-pane.collapsed ~ .spine ~ .after-pane {
    flex: 9 !important;
  }

  .before-pane:not(.collapsed) ~ .spine ~ .after-pane.collapsed {
    flex: 1 !important;
  }

  /* Override: when before is collapsed, after's flex is locked regardless of focus */
  .before-pane.collapsed ~ .spine ~ .after-pane.focused {
    flex: 9 !important;
  }

  /* Spine - chrome background, connectors draw on top */
  .spine {
    width: 24px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background-color: transparent;
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
    /* Re-enable scrollbars for islands */
    scrollbar-width: thin;
    scrollbar-color: var(--scrollbar-thumb) transparent;
  }

  .code-container::-webkit-scrollbar {
    width: 8px;
    height: 8px;
  }

  .code-container::-webkit-scrollbar-track {
    background: transparent;
  }

  .code-container::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb);
    border-radius: 4px;
  }

  .code-container::-webkit-scrollbar-thumb:hover {
    background: var(--scrollbar-thumb-hover);
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

  /* Changed line highlight - neutral tint that works for both sides */
  .line.content-changed {
    background-color: var(--diff-changed-bg);
  }

  /* Range boundary markers - visible but not distracting */
  .line.range-start::before {
    content: '';
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 1px;
    background-color: var(--diff-range-border);
  }

  .line.range-end::after {
    content: '';
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    height: 1px;
    background-color: var(--diff-range-border);
  }

  .line.range-hovered {
    background-color: var(--bg-hover);
  }

  .empty-state,
  .binary-notice {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: var(--size-lg);
    background-color: var(--bg-primary);
    border-radius: 12px;
  }

  /* Empty pane notice - centered, subtle styling */
  .empty-pane-notice {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    min-height: 200px;
  }

  .empty-pane-label {
    color: var(--text-faint);
    font-size: var(--size-sm);
    font-style: italic;
  }

  /* Range action toolbar */
  .range-toolbar {
    position: absolute;
    display: flex;
    gap: 1px;
    transform: translateY(-100%);
    z-index: 100;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
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
    background-color: var(--bg-hover);
  }

  .range-btn.discard-btn:hover {
    color: var(--status-deleted);
  }
</style>
