<!--
  DiffViewer.svelte - Side-by-side diff display
  
  Renders a two-pane diff view with synchronized scrolling, syntax highlighting,
  and visual connectors between corresponding changed regions. Supports panel
  minimization for new/deleted files, range-level discard operations, and comments.
  
  Alignments are loaded progressively to keep the UI responsive for large files.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X, GitBranch, MessageSquarePlus, MessageSquare, Trash2 } from 'lucide-svelte';
  import type { FileDiff, Alignment, Comment, Selection } from './types';
  import {
    commentsState,
    getCommentsForRange,
    addComment,
    updateComment,
    deleteComment,
  } from './stores/comments.svelte';
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

  // Comment state
  let commentingOnRange: number | null = $state(null);
  // Comment editor positioning: anchored to line, repositions on scroll
  // position: 'above' | 'below' determines which side of the range
  let commentEditorStyle: {
    top: number;
    left: number;
    width: number;
    position: 'above' | 'below';
    visible: boolean; // false when scrolled out of view
  } | null = $state(null);

  // Line selection state (for commenting on specific lines)
  // Selection is always on one pane at a time
  let lineSelection: {
    pane: 'before' | 'after';
    anchorLine: number; // where selection started
    focusLine: number; // where selection currently ends (can be < anchor)
  } | null = $state(null);
  let isSelecting = $state(false); // true during drag
  let justFinishedSelecting = $state(false); // flag to skip click after drag

  // Derived: normalized selection range (start <= end)
  let selectedLineRange = $derived.by(() => {
    if (!lineSelection) return null;
    const start = Math.min(lineSelection.anchorLine, lineSelection.focusLine);
    const end = Math.max(lineSelection.anchorLine, lineSelection.focusLine);
    return { pane: lineSelection.pane, start, end };
  });

  // Commenting on selected lines (separate from range comments)
  let commentingOnLines: { pane: 'before' | 'after'; start: number; end: number } | null =
    $state(null);
  let lineCommentEditorStyle: {
    top: number;
    left: number;
    width: number;
    visible: boolean;
  } | null = $state(null);

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
    updateCommentEditorPosition();
    updateLineSelectionToolbar();
    updateLineCommentEditorPosition();
  }

  function handleAfterScroll(e: Event) {
    if (!diff) return;
    const target = e.target as HTMLDivElement;
    scrollSync.onScroll('after', target, beforePane);
    redrawConnectors();
    updateToolbarPosition();
    updateCommentEditorPosition();
    updateLineSelectionToolbar();
    updateLineCommentEditorPosition();
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
      alignmentsWithComments,
    });
  }

  // Redraw connectors and update toolbar when panel size state changes
  // Use requestAnimationFrame loop during transition for smooth tracking
  $effect(() => {
    const _ = [beforeCollapsed, afterCollapsed, beforeHovered, afterHovered, spaceHeld];

    const startTime = performance.now();
    let rafId: number;

    function animateUpdate() {
      redrawConnectors();
      updateToolbarPosition();

      // Continue updating until transition completes
      if (performance.now() - startTime < PANEL_TRANSITION_MS) {
        rafId = requestAnimationFrame(animateUpdate);
      }
    }

    // Start the animation loop
    rafId = requestAnimationFrame(animateUpdate);

    // Cleanup on effect re-run
    return () => {
      cancelAnimationFrame(rafId);
    };
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

  // Redraw connectors when comments change (to update comment indicators)
  $effect(() => {
    // Depend on alignmentsWithComments
    const _ = alignmentsWithComments;
    if (diff && connectorSvg && beforePane) {
      redrawConnectors();
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

  // ==========================================================================
  // Comment handling
  // ==========================================================================

  // Get the current file path for comments
  let currentFilePath = $derived(afterPath ?? beforePath ?? '');

  // Get comments for the current alignment being hovered
  function getCommentsForAlignment(alignmentIndex: number): Comment[] {
    const alignmentData = changedAlignments[alignmentIndex];
    if (!alignmentData) return [];
    const { alignment } = alignmentData;
    // Use the after span for comment positioning
    return getCommentsForRange(alignment.after.start, alignment.after.end);
  }

  // Check if an alignment has comments
  function alignmentHasComments(alignmentIndex: number): boolean {
    return getCommentsForAlignment(alignmentIndex).length > 0;
  }

  // Compute set of alignment indices that have comments (for spine indicators)
  let alignmentsWithComments = $derived.by(() => {
    const set = new Set<number>();
    for (let i = 0; i < changedAlignments.length; i++) {
      if (alignmentHasComments(i)) {
        set.add(i);
      }
    }
    return set;
  });

  // Track whether comment should be above or below (decided once when opening)
  let commentPositionPreference: 'above' | 'below' = 'below';

  function handleStartComment() {
    if (hoveredRangeIndex === null) return;
    commentingOnRange = hoveredRangeIndex;

    // Decide position preference based on available space when opening
    commentPositionPreference = decideCommentPosition();
    updateCommentEditorPosition();
  }

  /**
   * Decide whether to position comment above or below based on available space.
   * Called once when opening the comment editor.
   */
  function decideCommentPosition(): 'above' | 'below' {
    if (commentingOnRange === null || !afterPane || !diffViewerEl) return 'below';

    const alignmentData = changedAlignments[commentingOnRange];
    if (!alignmentData) return 'below';

    const { alignment } = alignmentData;
    const paneRect = afterPane.getBoundingClientRect();
    const editorHeight = 120; // Approximate height of comment editor

    // Get the last line of the range
    const lastLineIndex = Math.max(alignment.after.start, alignment.after.end - 1);
    const lastLineEl = afterPane.querySelectorAll('.line')[lastLineIndex] as HTMLElement | null;
    if (!lastLineEl) return 'below';

    const lastLineRect = lastLineEl.getBoundingClientRect();
    const spaceBelow = paneRect.bottom - lastLineRect.bottom;

    // Get the first line of the range
    const firstLineEl = afterPane.querySelectorAll('.line')[
      alignment.after.start
    ] as HTMLElement | null;
    if (!firstLineEl) return 'below';

    const firstLineRect = firstLineEl.getBoundingClientRect();
    const spaceAbove = firstLineRect.top - paneRect.top;

    // Prefer below if there's enough space, otherwise above
    if (spaceBelow >= editorHeight) return 'below';
    if (spaceAbove >= editorHeight) return 'above';

    // If neither has enough space, pick the one with more
    return spaceBelow >= spaceAbove ? 'below' : 'above';
  }

  /**
   * Update comment editor position based on current scroll.
   * Called on every scroll to keep it anchored to the range.
   */
  function updateCommentEditorPosition() {
    if (commentingOnRange === null || !afterPane || !diffViewerEl) {
      commentEditorStyle = null;
      return;
    }

    const alignmentData = changedAlignments[commentingOnRange];
    if (!alignmentData) {
      commentEditorStyle = null;
      return;
    }

    const { alignment } = alignmentData;
    const viewerRect = diffViewerEl.getBoundingClientRect();
    const paneRect = afterPane.getBoundingClientRect();
    const editorHeight = 120;

    let top: number;
    let anchorLineEl: HTMLElement | null;

    if (commentPositionPreference === 'below') {
      // Anchor to bottom of last line
      const lastLineIndex = Math.max(alignment.after.start, alignment.after.end - 1);
      anchorLineEl = afterPane.querySelectorAll('.line')[lastLineIndex] as HTMLElement | null;
      if (!anchorLineEl) {
        commentEditorStyle = null;
        return;
      }
      const lineRect = anchorLineEl.getBoundingClientRect();
      top = lineRect.bottom - viewerRect.top;
    } else {
      // Anchor to top of first line
      anchorLineEl = afterPane.querySelectorAll('.line')[
        alignment.after.start
      ] as HTMLElement | null;
      if (!anchorLineEl) {
        commentEditorStyle = null;
        return;
      }
      const lineRect = anchorLineEl.getBoundingClientRect();
      top = lineRect.top - viewerRect.top - editorHeight;
    }

    // Check if the editor would be visible in the pane's scroll area
    // The pane content area starts after the header
    const paneContentTop = paneRect.top - viewerRect.top;
    const paneContentBottom = paneRect.bottom - viewerRect.top;

    // Determine visibility: editor is visible if any part is in the pane content area
    const editorTop = top;
    const editorBottom = top + editorHeight;
    const visible = editorBottom > paneContentTop && editorTop < paneContentBottom;

    commentEditorStyle = {
      top,
      left: paneRect.left - viewerRect.left + 12,
      width: paneRect.width - 24,
      position: commentPositionPreference,
      visible,
    };
  }

  async function handleCommentSubmit(content: string) {
    if (commentingOnRange === null || !currentFilePath) return;

    const alignmentData = changedAlignments[commentingOnRange];
    if (!alignmentData) return;

    const { alignment } = alignmentData;
    const selection: Selection = {
      type: 'range',
      span: { start: alignment.after.start, end: alignment.after.end },
    };

    await addComment(currentFilePath, selection, content);
    commentingOnRange = null;
    commentEditorStyle = null;
  }

  function handleCommentCancel() {
    commentingOnRange = null;
    commentEditorStyle = null;
  }

  async function handleCommentEdit(id: string, content: string) {
    await updateComment(id, content);
  }

  async function handleCommentDelete(id: string) {
    await deleteComment(id);
  }

  /**
   * Svelte action to auto-focus textarea.
   */
  function autoFocus(node: HTMLTextAreaElement) {
    node.focus();
  }

  /**
   * Check if a line is within a changed alignment (for highlighting).
   */
  function isLineInChangedAlignment(side: 'before' | 'after', lineIndex: number): boolean {
    const map = side === 'before' ? beforeLineToAlignment : afterLineToAlignment;
    return map.has(lineIndex);
  }

  /**
   * Check if a line is within the current selection.
   */
  function isLineSelected(pane: 'before' | 'after', lineIndex: number): boolean {
    if (!selectedLineRange || selectedLineRange.pane !== pane) return false;
    return lineIndex >= selectedLineRange.start && lineIndex <= selectedLineRange.end;
  }

  // ==========================================================================
  // Line selection handling
  // ==========================================================================

  function handleLineMouseDown(pane: 'before' | 'after', lineIndex: number, event: MouseEvent) {
    // Only handle left click
    if (event.button !== 0) return;

    // Prevent native text selection
    event.preventDefault();

    // Clear any existing text selection
    window.getSelection()?.removeAllRanges();

    // Start selection
    lineSelection = { pane, anchorLine: lineIndex, focusLine: lineIndex };
    isSelecting = true;

    // Clear any existing comment state
    commentingOnLines = null;
    lineCommentEditorStyle = null;

    // Add document-level mousemove listener for drag selection
    document.addEventListener('mousemove', handleDragMove);
  }

  function handleDragMove(event: MouseEvent) {
    if (!isSelecting || !lineSelection) return;

    // Find which line the mouse is over
    const pane = lineSelection.pane === 'before' ? beforePane : afterPane;
    if (!pane) return;

    const lineElements = pane.querySelectorAll('.line');
    for (let i = 0; i < lineElements.length; i++) {
      const rect = lineElements[i].getBoundingClientRect();
      if (event.clientY >= rect.top && event.clientY < rect.bottom) {
        if (lineSelection.focusLine !== i) {
          lineSelection = { ...lineSelection, focusLine: i };
        }
        break;
      }
    }
  }

  function handleLineMouseUp() {
    if (!isSelecting) return;
    isSelecting = false;
    // Set flag to skip the click event that fires after mouseup
    justFinishedSelecting = true;

    // Remove document-level mousemove listener
    document.removeEventListener('mousemove', handleDragMove);

    // If we have a valid selection, show the toolbar
    // Use requestAnimationFrame to ensure derived state is updated
    if (lineSelection) {
      requestAnimationFrame(() => {
        updateLineSelectionToolbar();
      });
    }
  }

  function clearLineSelection() {
    lineSelection = null;
    isSelecting = false;
    commentingOnLines = null;
    lineCommentEditorStyle = null;
  }

  // Track toolbar position for line selection
  let lineSelectionToolbarStyle: { top: number; left: number } | null = $state(null);

  function updateLineSelectionToolbar() {
    if (!selectedLineRange || !diffViewerEl) {
      lineSelectionToolbarStyle = null;
      return;
    }

    const pane = selectedLineRange.pane === 'before' ? beforePane : afterPane;
    if (!pane) {
      lineSelectionToolbarStyle = null;
      return;
    }

    // Position at the first selected line
    const lineEl = pane.querySelectorAll('.line')[selectedLineRange.start] as HTMLElement | null;
    if (!lineEl) {
      lineSelectionToolbarStyle = null;
      return;
    }

    const lineRect = lineEl.getBoundingClientRect();
    const viewerRect = diffViewerEl.getBoundingClientRect();

    lineSelectionToolbarStyle = {
      top: lineRect.top - viewerRect.top,
      left: lineRect.left - viewerRect.left,
    };
  }

  function handleStartLineComment() {
    if (!selectedLineRange) return;

    commentingOnLines = { ...selectedLineRange };
    updateLineCommentEditorPosition();
  }

  function updateLineCommentEditorPosition() {
    if (!commentingOnLines || !diffViewerEl) {
      lineCommentEditorStyle = null;
      return;
    }

    const pane = commentingOnLines.pane === 'before' ? beforePane : afterPane;
    if (!pane) {
      lineCommentEditorStyle = null;
      return;
    }

    const viewerRect = diffViewerEl.getBoundingClientRect();
    const paneRect = pane.getBoundingClientRect();

    // Position below the last selected line
    const lastLineEl = pane.querySelectorAll('.line')[commentingOnLines.end] as HTMLElement | null;
    if (!lastLineEl) {
      lineCommentEditorStyle = null;
      return;
    }

    const lineRect = lastLineEl.getBoundingClientRect();
    const top = lineRect.bottom - viewerRect.top;

    // Check visibility
    const editorHeight = 120;
    const paneContentTop = paneRect.top - viewerRect.top;
    const paneContentBottom = paneRect.bottom - viewerRect.top;
    const visible = top + editorHeight > paneContentTop && top < paneContentBottom;

    lineCommentEditorStyle = {
      top,
      left: paneRect.left - viewerRect.left + 12,
      width: paneRect.width - 24,
      visible,
    };
  }

  async function handleLineCommentSubmit(content: string) {
    if (!commentingOnLines || !currentFilePath) return;

    // Create selection based on whether it's a single line or range
    const selection: Selection =
      commentingOnLines.start === commentingOnLines.end
        ? { type: 'line', line: commentingOnLines.start }
        : {
            type: 'range',
            span: { start: commentingOnLines.start, end: commentingOnLines.end + 1 },
          };

    await addComment(currentFilePath, selection, content);
    clearLineSelection();
  }

  function handleLineCommentCancel() {
    commentingOnLines = null;
    lineCommentEditorStyle = null;
  }

  // Update toolbar position on scroll
  $effect(() => {
    if (selectedLineRange && !commentingOnLines) {
      // Re-run when scroll happens (tracked via beforePane/afterPane scroll)
      updateLineSelectionToolbar();
    }
  });

  // Update line comment editor position on scroll
  $effect(() => {
    if (commentingOnLines) {
      updateLineCommentEditorPosition();
    }
  });

  // Global mouseup handler to end selection even if mouse leaves the pane
  function handleGlobalMouseUp() {
    if (isSelecting) {
      handleLineMouseUp();
    }
  }

  // Handle click outside to clear selection
  function handleGlobalClick(event: MouseEvent) {
    // Skip the click event that fires immediately after finishing a drag selection
    if (justFinishedSelecting) {
      justFinishedSelecting = false;
      return;
    }

    // Don't clear if clicking on toolbar or comment editor
    const target = event.target as HTMLElement;
    if (
      target.closest('.line-selection-toolbar') ||
      target.closest('.line-comment-editor') ||
      target.closest('.line')
    ) {
      return;
    }

    // Clear selection if clicking elsewhere
    if (lineSelection && !isSelecting) {
      clearLineSelection();
    }
  }

  /**
   * Handle copy event to properly include newlines between lines.
   * The browser's default copy doesn't add newlines between div elements.
   * Also handles copying our custom line selection.
   */
  function handleCopy(event: ClipboardEvent) {
    // If we have a line selection, use that
    if (selectedLineRange) {
      event.preventDefault();
      const pane = selectedLineRange.pane === 'before' ? beforePane : afterPane;
      if (!pane) return;

      const lines: string[] = [];
      const lineElements = pane.querySelectorAll('.line');

      for (let i = selectedLineRange.start; i <= selectedLineRange.end; i++) {
        const lineEl = lineElements[i];
        if (lineEl) {
          const contentEl = lineEl.querySelector('.line-content');
          if (contentEl) {
            lines.push(contentEl.textContent || '');
          }
        }
      }

      if (lines.length > 0) {
        const text = lines.join('\n');
        event.clipboardData?.setData('text/plain', text);
      }
      return;
    }

    // Fall back to browser selection
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed) return;

    // Check if selection is within one of our code containers
    const range = selection.getRangeAt(0);
    const container = range.commonAncestorContainer;
    const codeContainer = (
      container instanceof Element ? container : container.parentElement
    )?.closest('.code-container');

    if (!codeContainer) return; // Not in our diff panes

    // Get all selected line elements
    const lines: string[] = [];
    const lineElements = codeContainer.querySelectorAll('.line');

    for (const lineEl of lineElements) {
      if (selection.containsNode(lineEl, true)) {
        // Get the text content of the line-content span
        const contentEl = lineEl.querySelector('.line-content');
        if (contentEl) {
          lines.push(contentEl.textContent || '');
        }
      }
    }

    if (lines.length > 0) {
      event.preventDefault();
      const text = lines.join('\n');
      event.clipboardData?.setData('text/plain', text);
    }
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

    // Copy handler for proper newline handling
    document.addEventListener('copy', handleCopy);

    // Global mouse handlers for line selection
    document.addEventListener('mouseup', handleGlobalMouseUp);
    document.addEventListener('click', handleGlobalClick);

    return () => {
      cleanupSpaceKey();
      cleanupKeyboardNav?.();
      document.removeEventListener('copy', handleCopy);
      document.removeEventListener('mouseup', handleGlobalMouseUp);
      document.removeEventListener('click', handleGlobalClick);
      document.removeEventListener('mousemove', handleDragMove);
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
              {@const isSelected = isLineSelected('before', i)}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="line"
                class:range-start={boundary.isStart}
                class:range-end={boundary.isEnd}
                class:range-hovered={isInHoveredRange}
                class:content-changed={isChanged}
                class:line-selected={isSelected}
                onmouseenter={() => handleLineMouseEnter('before', i)}
                onmouseleave={handleLineMouseLeave}
                onmousedown={(e) => handleLineMouseDown('before', i, e)}
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
              {@const isSelected = isLineSelected('after', i)}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="line"
                class:range-start={boundary.isStart}
                class:range-end={boundary.isEnd}
                class:range-hovered={isInHoveredRange}
                class:content-changed={isChanged}
                class:line-selected={isSelected}
                onmouseenter={() => handleLineMouseEnter('after', i)}
                onmouseleave={handleLineMouseLeave}
                onmousedown={(e) => handleLineMouseDown('after', i, e)}
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

    <!-- Range action toolbar (floating) -->
    {#if hoveredRangeIndex !== null && rangeToolbarStyle && commentingOnRange === null}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="range-toolbar"
        style="top: {rangeToolbarStyle.top}px; left: {rangeToolbarStyle.left}px;"
        onmouseleave={handleToolbarMouseLeave}
      >
        <button class="range-btn comment-btn" onclick={handleStartComment} title="Add comment (c)">
          {#if alignmentHasComments(hoveredRangeIndex)}
            <MessageSquare size={12} />
          {:else}
            <MessageSquarePlus size={12} />
          {/if}
        </button>
        {#if canDiscard}
          <button
            class="range-btn discard-btn"
            onclick={handleDiscardRange}
            title="Discard changes"
          >
            <X size={12} />
          </button>
        {/if}
      </div>
    {/if}

    <!-- Comment editor (sticky, anchored to range) -->
    {#if commentingOnRange !== null && commentEditorStyle}
      {@const existingComments = getCommentsForAlignment(commentingOnRange)}
      {@const existingComment = existingComments[0] ?? null}
      <div
        class="comment-editor"
        class:comment-editor-hidden={!commentEditorStyle.visible}
        style="top: {commentEditorStyle.top}px; left: {commentEditorStyle.left}px; width: {commentEditorStyle.width}px;"
      >
        <textarea
          class="comment-textarea"
          placeholder="Add a comment..."
          value={existingComment?.content ?? ''}
          onkeydown={(e) => {
            if (e.key === 'Escape') {
              handleCommentCancel();
            } else if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              const content = (e.target as HTMLTextAreaElement).value.trim();
              if (content) {
                if (existingComment) {
                  handleCommentEdit(existingComment.id, content);
                } else {
                  handleCommentSubmit(content);
                }
              }
              handleCommentCancel();
            }
          }}
          use:autoFocus
        ></textarea>
        <div class="comment-editor-hint">
          <span>Enter to save · Esc to cancel</span>
          {#if existingComment}
            <button
              class="delete-comment-btn"
              onclick={() => {
                handleCommentDelete(existingComment.id);
                handleCommentCancel();
              }}
              title="Delete comment"
            >
              <Trash2 size={12} />
            </button>
          {/if}
        </div>
      </div>
    {/if}

    <!-- Line selection toolbar (floating) -->
    {#if selectedLineRange && lineSelectionToolbarStyle && !commentingOnLines}
      <div
        class="line-selection-toolbar"
        style="top: {lineSelectionToolbarStyle.top}px; left: {lineSelectionToolbarStyle.left}px;"
      >
        <span class="selection-info">
          {selectedLineRange.end - selectedLineRange.start + 1} line{selectedLineRange.end !==
          selectedLineRange.start
            ? 's'
            : ''}
        </span>
        <button class="range-btn comment-btn" onclick={handleStartLineComment} title="Add comment">
          <MessageSquarePlus size={12} />
        </button>
        <button class="range-btn" onclick={clearLineSelection} title="Clear selection">
          <X size={12} />
        </button>
      </div>
    {/if}

    <!-- Line comment editor -->
    {#if commentingOnLines && lineCommentEditorStyle}
      <div
        class="comment-editor line-comment-editor"
        class:comment-editor-hidden={!lineCommentEditorStyle.visible}
        style="top: {lineCommentEditorStyle.top}px; left: {lineCommentEditorStyle.left}px; width: {lineCommentEditorStyle.width}px;"
      >
        <textarea
          class="comment-textarea"
          placeholder="Add a comment on {commentingOnLines.end -
            commentingOnLines.start +
            1} line{commentingOnLines.end !== commentingOnLines.start ? 's' : ''}..."
          onkeydown={(e) => {
            if (e.key === 'Escape') {
              handleLineCommentCancel();
            } else if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              const content = (e.target as HTMLTextAreaElement).value.trim();
              if (content) {
                handleLineCommentSubmit(content);
              } else {
                handleLineCommentCancel();
              }
            }
          }}
          use:autoFocus
        ></textarea>
        <div class="comment-editor-hint">
          <span>Enter to save · Esc to cancel</span>
        </div>
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

  /* Line selection highlight */
  .line.line-selected {
    background-color: var(--accent-primary-muted, rgba(59, 130, 246, 0.15));
  }

  /* Selection takes precedence over other highlights */
  .line.line-selected.content-changed,
  .line.line-selected.range-hovered {
    background-color: var(--accent-primary-muted, rgba(59, 130, 246, 0.15));
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

  .range-btn.comment-btn:hover {
    color: var(--accent-primary);
  }

  /* Comment editor - flat, clean design, sticky to range */
  .comment-editor {
    position: absolute;
    z-index: 100;
    display: flex;
    flex-direction: column;
    background-color: var(--bg-chrome);
    border-radius: 8px;
    overflow: hidden;
    /* Smooth position updates during scroll */
    transition: opacity 0.15s ease;
  }

  /* Hidden when scrolled out of view */
  .comment-editor-hidden {
    opacity: 0.3;
    pointer-events: none;
  }

  .comment-textarea {
    width: 100%;
    height: 84px; /* 4 lines (14px font * 1.5 line-height * 4) */
    padding: 10px 12px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-family: inherit;
    font-size: var(--size-sm);
    line-height: 1.5;
    resize: none;
    overflow-y: auto;
  }

  .comment-textarea:focus {
    outline: none;
  }

  .comment-textarea::placeholder {
    color: var(--text-faint);
  }

  .comment-editor-hint {
    display: flex;
    align-items: center;
    padding: 4px 12px 8px;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .delete-comment-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    margin-left: auto;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .delete-comment-btn:hover {
    color: var(--status-deleted);
    background-color: var(--bg-hover);
  }

  /* Line selection toolbar */
  .line-selection-toolbar {
    position: absolute;
    display: flex;
    align-items: center;
    gap: 4px;
    transform: translateY(-100%);
    z-index: 100;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-bottom: none;
    border-radius: 4px 4px 0 0;
    padding: 0 4px;
  }

  .selection-info {
    font-size: var(--size-xs);
    color: var(--text-muted);
    padding: 4px 4px;
    white-space: nowrap;
  }

  /* Prevent text selection during line drag */
  .code-container {
    user-select: none;
  }
</style>
