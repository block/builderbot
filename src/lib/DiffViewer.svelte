<!--
  DiffViewer.svelte - Unified diff display
  
  Handles three display modes:
  1. Two-pane diff: Side-by-side before/after with synchronized scrolling and spine connectors
  2. Created file: Status label + spine + single after pane (commentable)
  3. Deleted file: Single before pane + spine + status label
  
  The spine is always present - it shows bezier connectors for two-pane diffs,
  and comment highlights for all modes.
  
  Uses custom scroll implementation for frame-perfect sync between panes.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X, GitBranch, MessageSquarePlus, MessageSquare, Trash2 } from 'lucide-svelte';
  import type { FileDiff, Alignment, Comment, Span } from './types';
  import {
    commentsState,
    getCommentsForRange,
    getCommentsForCurrentFile,
    findCommentById,
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
  import { createScrollController } from './services/scrollController.svelte';
  import {
    ConnectorRendererCanvas,
    type CommentHighlightInfo,
  } from './services/connectorRendererCanvas';
  import {
    getLineBoundary,
    getLanguageFromDiff,
    getFilePath,
    isBinaryDiff,
    getTextLines,
  } from './diffUtils';
  import { setupDiffKeyboardNav } from './diffKeyboard';
  import { diffSelection } from './stores/diffSelection.svelte';
  import { diffState, clearScrollTarget } from './stores/diffState.svelte';
  import { DiffSpec } from './types';
  import CommentEditor from './CommentEditor.svelte';
  import Scrollbar from './Scrollbar.svelte';

  // ==========================================================================
  // Constants
  // ==========================================================================

  /** Duration (ms) for panel flex transitions - used to schedule connector redraws */
  const PANEL_TRANSITION_MS = 250;

  // ==========================================================================
  // Props
  // ==========================================================================

  interface Props {
    diff: FileDiff | null;
    sizeBase?: number;
    /** Bumped when syntax theme changes to trigger re-highlight */
    syntaxThemeVersion?: number;
    /** Whether a new file is loading (show subtle indicator, keep old content) */
    loading?: boolean;
    /** Whether this is a reference file (not part of the diff) */
    isReferenceFile?: boolean;
    onRangeDiscard?: () => void;
  }

  let {
    diff,
    sizeBase,
    syntaxThemeVersion = 0,
    loading = false,
    isReferenceFile = false,
    onRangeDiscard,
  }: Props = $props();

  // Get diff spec from store for display and logic
  let isWorkingTree = $derived(diffSelection.spec.head.type === 'WorkingTree');
  let diffBaseDisplay = $derived(
    diffSelection.spec.base.type === 'WorkingTree' ? '@' : diffSelection.spec.base.value
  );
  let diffHeadDisplay = $derived(
    diffSelection.spec.head.type === 'WorkingTree' ? 'Working Tree' : diffSelection.spec.head.value
  );

  // ==========================================================================
  // Element refs
  // ==========================================================================

  let beforePane: HTMLDivElement | null = $state(null);
  let afterPane: HTMLDivElement | null = $state(null);
  let connectorCanvas: HTMLCanvasElement | null = $state(null);
  let diffViewerEl: HTMLDivElement | null = $state(null);

  // ==========================================================================
  // Highlighter state
  // ==========================================================================

  let highlighterReady = $state(false);
  let languageReady = $state(false);
  let beforeTokens: Token[][] = $state([]);
  let afterTokens: Token[][] = $state([]);

  // ==========================================================================
  // Panel state (two-pane mode only)
  // ==========================================================================

  /** Ratio of before pane width (0-1). 0.4 = 40% before, 60% after */
  let paneRatio = $state(0.4);

  /** Whether user is currently dragging the divider */
  let isDraggingDivider = $state(false);

  // ==========================================================================
  // Range hover state (for toolbar on changed ranges)
  // ==========================================================================

  let hoveredRangeIndex: number | null = $state(null);
  let rangeToolbarStyle: { top: number; left: number } | null = $state(null);

  // ==========================================================================
  // Comment state
  // ==========================================================================

  // Range-based commenting (from alignment hover)
  let commentingOnRange: number | null = $state(null);
  let commentEditorStyle: {
    top: number;
    left: number;
    width: number;
    position: 'above' | 'below';
    visible: boolean;
  } | null = $state(null);
  let commentPositionPreference: 'above' | 'below' = 'below';

  // Line-based commenting (from line selection)
  let lineSelection: {
    pane: 'before' | 'after';
    anchorLine: number;
    focusLine: number;
  } | null = $state(null);
  let isSelecting = $state(false);
  let justFinishedSelecting = $state(false);

  let commentingOnLines: { pane: 'before' | 'after'; start: number; end: number } | null =
    $state(null);
  let lineCommentEditorStyle: {
    top: number;
    left: number;
    width: number;
    visible: boolean;
  } | null = $state(null);
  let editingCommentId: string | null = $state(null);
  let lineSelectionToolbarStyle: { top: number; left: number } | null = $state(null);

  // ==========================================================================
  // Derived state
  // ==========================================================================

  // Normalized selection range (start <= end)
  let selectedLineRange = $derived.by(() => {
    if (!lineSelection) return null;
    const start = Math.min(lineSelection.anchorLine, lineSelection.focusLine);
    const end = Math.max(lineSelection.anchorLine, lineSelection.focusLine);
    return { pane: lineSelection.pane, start, end };
  });

  // Alignments from the current diff
  let activeAlignments = $derived(diff?.alignments ?? []);

  // File type detection
  let isNewFile = $derived(diff !== null && diff.before === null);
  let isDeletedFile = $derived(diff !== null && diff.after === null);
  let isTwoPaneMode = $derived(!isNewFile && !isDeletedFile);
  let isBinary = $derived(diff !== null && isBinaryDiff(diff));

  // Check if alignment loading is complete
  let alignmentsFullyLoaded = $derived(
    diff !== null && activeAlignments.length >= diff.alignments.length
  );

  // Discard is only available when viewing the working tree
  let canDiscard = $derived(isWorkingTree);

  // Extract lines from the diff
  let beforeLines = $derived(diff ? getTextLines(diff, 'before') : []);
  let afterLines = $derived(diff ? getTextLines(diff, 'after') : []);

  // File paths
  let beforePath = $derived(diff?.before?.path ?? null);
  let afterPath = $derived(diff?.after?.path ?? null);
  let currentFilePath = $derived(afterPath ?? beforePath ?? '');

  // Language detection
  let language = $derived(diff ? getLanguageFromDiff(diff, detectLanguage) : null);

  // Show range markers only in two-pane mode
  let showRangeMarkers = $derived(isTwoPaneMode);

  // Changed alignments with indices
  let changedAlignments = $derived(
    activeAlignments
      .map((alignment, index) => ({ alignment, index }))
      .filter(({ alignment }) => alignment.changed)
  );

  // Line-to-alignment maps for hover detection
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

  // Comments for current file
  let currentFileComments = $derived.by(() => {
    if (!commentsState.currentPath) return [];
    return commentsState.comments.filter((c) => c.path === commentsState.currentPath);
  });

  // ==========================================================================
  // Custom scroll controller (frame-perfect sync)
  // ==========================================================================

  const scrollController = createScrollController();

  // Update scroll controller with active alignments
  // Pass file path so scroll only resets on file change, not content refresh
  $effect(() => {
    const filePath = diff ? getFilePath(diff) : null;
    scrollController.setAlignments(activeAlignments, filePath);
  });

  // Measure line height from DOM
  function measureLineHeight(pane: HTMLElement | null): number {
    if (!pane) return 20;
    const firstLine = pane.querySelector('.line') as HTMLElement | null;
    return firstLine ? firstLine.getBoundingClientRect().height : 20;
  }

  // Update dimensions when panes are available or content changes
  $effect(() => {
    if (beforePane && beforeLines.length > 0) {
      const lineHeight = measureLineHeight(beforePane);
      scrollController.setDimensions('before', {
        viewportHeight: beforePane.clientHeight,
        contentHeight: beforeLines.length * lineHeight,
        lineHeight,
      });
    }
  });

  $effect(() => {
    if (afterPane && afterLines.length > 0) {
      const lineHeight = measureLineHeight(afterPane);
      scrollController.setDimensions('after', {
        viewportHeight: afterPane.clientHeight,
        contentHeight: afterLines.length * lineHeight,
        lineHeight,
      });
    }
  });

  // Scrollbar marker computation
  let beforeMarkers = $derived.by(() => {
    if (beforeLines.length === 0) return [];
    return changedAlignments.map(({ alignment }) => {
      const span = alignment.before;
      const startPercent = (span.start / beforeLines.length) * 100;
      const rangeSize = span.end - span.start;
      const heightPercent = Math.max(0.5, (rangeSize / beforeLines.length) * 100);
      return { top: startPercent, height: heightPercent, type: 'change' as const };
    });
  });

  let afterMarkers = $derived.by(() => {
    if (afterLines.length === 0) return [];
    const changeMarkers = changedAlignments.map(({ alignment }) => {
      const span = alignment.after;
      const startPercent = (span.start / afterLines.length) * 100;
      const rangeSize = span.end - span.start;
      const heightPercent = Math.max(0.5, (rangeSize / afterLines.length) * 100);
      return { top: startPercent, height: heightPercent, type: 'change' as const };
    });

    const commentMarkers = currentFileComments
      .filter((c) => c.span.start !== 0 || c.span.end !== 0)
      .map((comment) => {
        const startPercent = (comment.span.start / afterLines.length) * 100;
        const rangeSize = Math.max(1, comment.span.end - comment.span.start);
        const heightPercent = Math.max(0.5, (rangeSize / afterLines.length) * 100);
        return { top: startPercent, height: heightPercent, type: 'comment' as const };
      });

    return [...changeMarkers, ...commentMarkers];
  });

  // Content dimensions for scrollbars
  let beforeContentHeight = $derived(beforeLines.length * (measureLineHeight(beforePane) || 20));
  let afterContentHeight = $derived(afterLines.length * (measureLineHeight(afterPane) || 20));

  // ==========================================================================
  // Effects
  // ==========================================================================

  // Reset UI state on diff change
  $effect(() => {
    if (diff) {
      hoveredRangeIndex = null;
      rangeToolbarStyle = null;
      // Clear any line selection state from previous file
      lineSelection = null;
      commentingOnLines = null;
      lineCommentEditorStyle = null;
      editingCommentId = null;
      commentingOnRange = null;
      commentEditorStyle = null;
    }
  });

  // Syntax highlighting
  $effect(() => {
    const _version = syntaxThemeVersion;

    if (!diff) {
      beforeTokens = [];
      afterTokens = [];
      return;
    }

    if (highlighterReady && languageReady) {
      const beforeCode = beforeLines.join('\n');
      const afterCode = afterLines.join('\n');
      beforeTokens = beforeCode ? highlightLines(beforeCode, language) : [];
      afterTokens = afterCode ? highlightLines(afterCode, language) : [];
    } else {
      beforeTokens = beforeLines.map((line) => [{ content: line, color: 'inherit' }]);
      afterTokens = afterLines.map((line) => [{ content: line, color: 'inherit' }]);
    }
  });

  // Language preparation
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

  // ==========================================================================
  // Connector Renderer (high-performance Canvas rendering)
  // ==========================================================================

  let connectorRenderer: ConnectorRendererCanvas | null = $state(null);

  // Initialize renderer when Canvas is available (recreate if canvas element changes)
  let previousCanvas: HTMLCanvasElement | null = null;
  $effect(() => {
    if (connectorCanvas && connectorCanvas !== previousCanvas) {
      // Canvas element changed - destroy old renderer and create new one
      if (connectorRenderer) {
        connectorRenderer.destroy();
      }
      connectorRenderer = new ConnectorRendererCanvas(connectorCanvas, {
        onCommentClick: handleCommentHighlightClick,
      });
      previousCanvas = connectorCanvas;
      scheduleConnectorRedraw();
    }
  });

  // Update renderer alignments when diff changes
  $effect(() => {
    if (!connectorRenderer) return;

    if (!diff) {
      connectorRenderer.clear();
      return;
    }

    // In single-pane mode, pass empty alignments (no curves) but still draw comments
    const alignmentsForRenderer = isTwoPaneMode ? activeAlignments : [];
    connectorRenderer.setAlignments(alignmentsForRenderer);
    scheduleConnectorRedraw();
  });

  // Update renderer comments when they change
  $effect(() => {
    if (connectorRenderer) {
      connectorRenderer.setComments(currentFileComments);
      scheduleConnectorRedraw();
    }
  });

  // Update renderer hover state
  $effect(() => {
    if (connectorRenderer) {
      connectorRenderer.setHoveredIndex(hoveredRangeIndex);
      scheduleConnectorRedraw();
    }
  });

  // Update renderer colors when theme changes
  $effect(() => {
    const _version = syntaxThemeVersion;
    if (connectorRenderer) {
      connectorRenderer.updateColors();
      scheduleConnectorRedraw();
    }
  });

  // ==========================================================================
  // Connector drawing
  // ==========================================================================

  let connectorRedrawPending = false;

  function scheduleConnectorRedraw() {
    if (connectorRedrawPending) return;
    connectorRedrawPending = true;
    requestAnimationFrame(() => {
      connectorRedrawPending = false;
      redrawConnectorsImpl();
    });
  }

  function redrawConnectorsImpl() {
    if (!connectorRenderer || !afterPane || !diff) return;

    // For single-pane modes, we still draw comment highlights
    const sourcePane = beforePane ?? afterPane;
    const firstLine = sourcePane.querySelector('.line') as HTMLElement | null;
    const lineHeight = firstLine ? firstLine.getBoundingClientRect().height : 20;

    const canvasRect = connectorCanvas?.getBoundingClientRect();
    const containerRect = afterPane.getBoundingClientRect();
    const verticalOffset = canvasRect ? containerRect.top - canvasRect.top : 0;

    // Use scroll controller positions (not native scrollTop since we use transform)
    connectorRenderer.render(
      scrollController.beforeScrollY,
      scrollController.afterScrollY,
      lineHeight,
      verticalOffset
    );
  }

  function redrawConnectors() {
    redrawConnectorsImpl();
  }

  // Redraw when font size changes (sizeBase affects line height)
  $effect(() => {
    const _ = sizeBase;
    if (diff && connectorCanvas && afterPane) {
      scheduleConnectorRedraw();
    }
  });

  // ==========================================================================
  // Token helpers
  // ==========================================================================

  function getBeforeTokens(index: number): Token[] {
    return beforeTokens[index] || [{ content: '', color: 'inherit' }];
  }

  function getAfterTokens(index: number): Token[] {
    return afterTokens[index] || [{ content: '', color: 'inherit' }];
  }

  // ==========================================================================
  // Line state helpers
  // ==========================================================================

  function isLineInChangedAlignment(side: 'before' | 'after', lineIndex: number): boolean {
    const map = side === 'before' ? beforeLineToAlignment : afterLineToAlignment;
    return map.has(lineIndex);
  }

  function isLineSelected(pane: 'before' | 'after', lineIndex: number): boolean {
    if (!selectedLineRange || selectedLineRange.pane !== pane) return false;
    return lineIndex >= selectedLineRange.start && lineIndex <= selectedLineRange.end;
  }

  function isLineInHoveredRange(pane: 'before' | 'after', lineIndex: number): boolean {
    if (hoveredRangeIndex === null) return false;
    const map = pane === 'before' ? beforeLineToAlignment : afterLineToAlignment;
    return map.get(lineIndex) === hoveredRangeIndex;
  }

  // ==========================================================================
  // Comment helpers
  // ==========================================================================

  function getCommentsForAlignment(alignmentIndex: number): Comment[] {
    const alignmentData = changedAlignments[alignmentIndex];
    if (!alignmentData) return [];
    const { alignment } = alignmentData;
    return getCommentsForRange(alignment.after.start, alignment.after.end);
  }

  function alignmentHasComments(alignmentIndex: number): boolean {
    return getCommentsForAlignment(alignmentIndex).length > 0;
  }

  // ==========================================================================
  // Scroll handlers (custom scroll via wheel events)
  // ==========================================================================

  function handleWheel(side: 'before' | 'after', e: WheelEvent) {
    e.preventDefault();
    scrollController.scrollBy(side, e.deltaY);

    // Trigger UI updates
    redrawConnectors();
    updateToolbarPosition();
    updateCommentEditorPosition();
    updateLineSelectionToolbar();
    updateLineCommentEditorPosition();
  }

  function handleBeforeWheel(e: WheelEvent) {
    if (!diff) return;
    // Allow scrolling in two-pane mode and deleted file mode
    if (!isTwoPaneMode && !isDeletedFile) return;
    handleWheel('before', e);
  }

  function handleAfterWheel(e: WheelEvent) {
    if (!diff) return;
    handleWheel('after', e);
  }

  // Handle scrollbar callbacks
  function handleBeforeScrollbarScroll(deltaY: number) {
    scrollController.scrollBy('before', deltaY);
    redrawConnectors();
    updateToolbarPosition();
    updateCommentEditorPosition();
    updateLineSelectionToolbar();
    updateLineCommentEditorPosition();
  }

  function handleAfterScrollbarScroll(deltaY: number) {
    scrollController.scrollBy('after', deltaY);
    redrawConnectors();
    updateToolbarPosition();
    updateCommentEditorPosition();
    updateLineSelectionToolbar();
    updateLineCommentEditorPosition();
  }

  // Redraw connectors when scroll positions change
  $effect(() => {
    const _before = scrollController.beforeScrollY;
    const _after = scrollController.afterScrollY;
    if (diff && connectorCanvas && afterPane) {
      scheduleConnectorRedraw();
    }
  });

  // ==========================================================================
  // Divider drag handling
  // ==========================================================================

  function handleDividerMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    isDraggingDivider = true;
    document.addEventListener('mousemove', handleDividerMouseMove);
    document.addEventListener('mouseup', handleDividerMouseUp);
  }

  function handleDividerMouseMove(e: MouseEvent) {
    if (!isDraggingDivider || !diffViewerEl) return;

    const rect = diffViewerEl.getBoundingClientRect();
    // Account for left padding (8px) and spine width (24px)
    const availableWidth = rect.width - 8 - 24;
    const mouseX = e.clientX - rect.left - 8;

    // Calculate ratio, clamping to reasonable bounds (15% - 85%)
    let ratio = mouseX / availableWidth;
    ratio = Math.max(0.15, Math.min(0.85, ratio));

    paneRatio = ratio;

    // Update connectors during drag
    redrawConnectors();
  }

  function handleDividerMouseUp() {
    isDraggingDivider = false;
    document.removeEventListener('mousemove', handleDividerMouseMove);
    document.removeEventListener('mouseup', handleDividerMouseUp);
  }

  function handleDividerDoubleClick() {
    // Reset to default 40/60 split
    paneRatio = 0.4;
    redrawConnectors();
  }

  // Redraw connectors when pane ratio changes
  $effect(() => {
    const _ = paneRatio;
    if (diff && connectorCanvas && afterPane) {
      requestAnimationFrame(() => {
        scheduleConnectorRedraw();
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

    const lineIndex = alignmentData.alignment.after.start;
    const lineEl = afterPane.querySelectorAll('.line')[lineIndex] as HTMLElement | null;

    if (!lineEl) {
      rangeToolbarStyle = null;
      return;
    }

    const lineRect = lineEl.getBoundingClientRect();
    const viewerRect = diffViewerEl.getBoundingClientRect();

    rangeToolbarStyle = {
      top: lineRect.top - viewerRect.top,
      left: lineRect.left - viewerRect.left,
    };
  }

  function handleLineMouseEnter(pane: 'before' | 'after', lineIndex: number) {
    if (!isTwoPaneMode) return;
    const map = pane === 'before' ? beforeLineToAlignment : afterLineToAlignment;
    const alignmentIdx = map.get(lineIndex);

    if (alignmentIdx !== undefined) {
      hoveredRangeIndex = alignmentIdx;
      requestAnimationFrame(updateToolbarPosition);
    }
  }

  function handleLineMouseLeave(event: MouseEvent) {
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

    hoveredRangeIndex = null;
    rangeToolbarStyle = null;
    onRangeDiscard?.();
  }

  // ==========================================================================
  // Comment highlight click (from spine)
  // ==========================================================================

  function handleCommentHighlightClick(info: CommentHighlightInfo) {
    if (!afterPane) return;

    const { span, commentId } = info;
    scrollToLine(span.start);

    const start = span.start;
    const end = Math.max(span.start, span.end - 1);

    lineSelection = { pane: 'after', anchorLine: start, focusLine: end };
    commentingOnLines = { pane: 'after', start, end };
    editingCommentId = commentId;
    updateLineCommentEditorPosition();
  }

  function scrollToLine(lineIndex: number) {
    // Use the scroll controller which manages scrolling via CSS transforms
    scrollController.scrollToRow(lineIndex, 'after');
  }

  // ==========================================================================
  // Range comment handling
  // ==========================================================================

  function handleStartComment() {
    if (hoveredRangeIndex === null) return;
    commentingOnRange = hoveredRangeIndex;
    commentPositionPreference = decideCommentPosition();
    updateCommentEditorPosition();
  }

  function decideCommentPosition(): 'above' | 'below' {
    if (commentingOnRange === null || !afterPane || !diffViewerEl) return 'below';

    const alignmentData = changedAlignments[commentingOnRange];
    if (!alignmentData) return 'below';

    const { alignment } = alignmentData;
    const paneRect = afterPane.getBoundingClientRect();
    const editorHeight = 120;

    const lastLineIndex = Math.max(alignment.after.start, alignment.after.end - 1);
    const lastLineEl = afterPane.querySelectorAll('.line')[lastLineIndex] as HTMLElement | null;
    if (!lastLineEl) return 'below';

    const lastLineRect = lastLineEl.getBoundingClientRect();
    const spaceBelow = paneRect.bottom - lastLineRect.bottom;

    const firstLineEl = afterPane.querySelectorAll('.line')[
      alignment.after.start
    ] as HTMLElement | null;
    if (!firstLineEl) return 'below';

    const firstLineRect = firstLineEl.getBoundingClientRect();
    const spaceAbove = firstLineRect.top - paneRect.top;

    if (spaceBelow >= editorHeight) return 'below';
    if (spaceAbove >= editorHeight) return 'above';

    return spaceBelow >= spaceAbove ? 'below' : 'above';
  }

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
      const lastLineIndex = Math.max(alignment.after.start, alignment.after.end - 1);
      anchorLineEl = afterPane.querySelectorAll('.line')[lastLineIndex] as HTMLElement | null;
      if (!anchorLineEl) {
        commentEditorStyle = null;
        return;
      }
      const lineRect = anchorLineEl.getBoundingClientRect();
      top = lineRect.bottom - viewerRect.top;
    } else {
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

    const paneContentTop = paneRect.top - viewerRect.top;
    const paneContentBottom = paneRect.bottom - viewerRect.top;
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
    const span: Span = { start: alignment.after.start, end: alignment.after.end };

    await addComment(currentFilePath, span, content);
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

  // ==========================================================================
  // Line selection handling
  // ==========================================================================

  function handleLineMouseDown(pane: 'before' | 'after', lineIndex: number, event: MouseEvent) {
    // Only allow selection on after pane (commentable)
    if (pane === 'before') return;
    if (event.button !== 0) return;

    event.preventDefault();
    window.getSelection()?.removeAllRanges();

    lineSelection = { pane, anchorLine: lineIndex, focusLine: lineIndex };
    isSelecting = true;

    commentingOnLines = null;
    lineCommentEditorStyle = null;

    document.addEventListener('mousemove', handleDragMove);
  }

  function handleDragMove(event: MouseEvent) {
    if (!isSelecting || !lineSelection) return;

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
    justFinishedSelecting = true;

    document.removeEventListener('mousemove', handleDragMove);

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
    editingCommentId = null;
  }

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

    const lastLineEl = pane.querySelectorAll('.line')[commentingOnLines.end] as HTMLElement | null;
    if (!lastLineEl) {
      lineCommentEditorStyle = null;
      return;
    }

    const lineRect = lastLineEl.getBoundingClientRect();
    const top = lineRect.bottom - viewerRect.top;

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

    const span: Span = {
      start: commentingOnLines.start,
      end: commentingOnLines.end + 1,
    };

    await addComment(currentFilePath, span, content);
    clearLineSelection();
  }

  function handleLineCommentCancel() {
    commentingOnLines = null;
    lineCommentEditorStyle = null;
  }

  // Update toolbar/editor positions on scroll
  $effect(() => {
    if (selectedLineRange && !commentingOnLines) {
      updateLineSelectionToolbar();
    }
  });

  $effect(() => {
    if (commentingOnLines) {
      updateLineCommentEditorPosition();
    }
  });

  // ==========================================================================
  // Global event handlers
  // ==========================================================================

  function handleGlobalMouseUp() {
    if (isSelecting) {
      handleLineMouseUp();
    }
  }

  function handleGlobalClick(event: MouseEvent) {
    if (justFinishedSelecting) {
      justFinishedSelecting = false;
      return;
    }

    const target = event.target as HTMLElement;
    if (
      target.closest('.line-selection-toolbar') ||
      target.closest('.line-comment-editor') ||
      target.closest('.line')
    ) {
      return;
    }

    if (lineSelection && !isSelecting) {
      clearLineSelection();
    }
  }

  function handleLineSelectionKeydown(event: KeyboardEvent) {
    // Skip if focus is in an input or textarea
    const target = event.target as HTMLElement;
    if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') {
      return;
    }

    // Enter: Open comment dialog when lines are selected but dialog not open
    if (event.key === 'Enter' && selectedLineRange && !commentingOnLines) {
      event.preventDefault();
      handleStartLineComment();
      return;
    }

    // Escape: Clear line selection when lines are selected but dialog not open
    if (event.key === 'Escape' && selectedLineRange && !commentingOnLines) {
      event.preventDefault();
      clearLineSelection();
      return;
    }
  }

  function handleCopy(event: ClipboardEvent) {
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

    const selection = window.getSelection();
    if (!selection || selection.isCollapsed) return;

    const range = selection.getRangeAt(0);
    const container = range.commonAncestorContainer;
    const codeContainer = (
      container instanceof Element ? container : container.parentElement
    )?.closest('.code-container');

    if (!codeContainer) return;

    const lines: string[] = [];
    const lineElements = codeContainer.querySelectorAll('.line');

    for (const lineEl of lineElements) {
      if (selection.containsNode(lineEl, true)) {
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

  // ==========================================================================
  // Lifecycle
  // ==========================================================================

  onMount(() => {
    initHighlighter().then(() => {
      highlighterReady = true;
    });

    const cleanupKeyboardNav = setupDiffKeyboardNav({
      getChangedAlignments: () => changedAlignments,
      scrollToRow: (row, side) => scrollController.scrollToRow(row, side),
      scrollBy: (deltaY) => scrollController.scrollBy('after', deltaY),
      getCurrentScrollY: () => scrollController.afterScrollY,
      getLineHeight: () => scrollController.getDimensions('after').lineHeight,
      getViewportHeight: () => scrollController.getDimensions('after').viewportHeight,
    });

    document.addEventListener('copy', handleCopy);
    document.addEventListener('mouseup', handleGlobalMouseUp);
    document.addEventListener('click', handleGlobalClick);
    document.addEventListener('keydown', handleLineSelectionKeydown);

    return () => {
      cleanupKeyboardNav?.();
      document.removeEventListener('copy', handleCopy);
      document.removeEventListener('mouseup', handleGlobalMouseUp);
      document.removeEventListener('click', handleGlobalClick);
      document.removeEventListener('keydown', handleLineSelectionKeydown);
      document.removeEventListener('mousemove', handleDragMove);
      document.removeEventListener('mousemove', handleDividerMouseMove);
      document.removeEventListener('mouseup', handleDividerMouseUp);
      // Clean up connector renderer
      if (connectorRenderer) {
        connectorRenderer.destroy();
        connectorRenderer = null;
      }
    };
  });

  // Handle external scroll target requests (e.g., from sidebar comment clicks)
  $effect(() => {
    const targetLine = diffState.scrollTargetLine;
    // Wait until alignments are fully loaded before scrolling
    if (targetLine !== null && afterPane && diff && alignmentsFullyLoaded) {
      // Use requestAnimationFrame to ensure DOM is painted after alignment load
      requestAnimationFrame(() => {
        scrollToLine(targetLine);
        clearScrollTarget();
      });
    }
  });
</script>

<div class="diff-viewer" class:loading bind:this={diffViewerEl}>
  {#if loading}
    <div class="loading-overlay">
      <span class="loading-text">Loading...</span>
    </div>
  {/if}
  {#if diff === null}
    <div class="empty-state">
      <p>Select a file to view changes</p>
    </div>
  {:else if isBinary}
    <div class="binary-notice">
      <p>Binary file - cannot display diff</p>
    </div>
  {:else}
    <div class="diff-content" class:single-pane={!isTwoPaneMode}>
      <!-- Created/Reference file: label on left -->
      {#if isReferenceFile}
        <div class="status-label reference">
          <span class="status-text">Reference</span>
        </div>
      {:else if isNewFile}
        <div class="status-label created">
          <span class="status-text">Created</span>
        </div>
      {/if}

      <!-- Before pane (only in two-pane mode) -->
      {#if isTwoPaneMode}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="diff-pane before-pane" style="flex: {paneRatio}">
          <div class="pane-header">
            <span class="pane-ref">
              <GitBranch size={12} />
              {diffBaseDisplay}
            </span>
            <span class="pane-path" title={beforePath}>{beforePath ?? 'No file'}</span>
          </div>
          <div class="code-area" onwheel={handleBeforeWheel}>
            <Scrollbar
              scrollY={scrollController.beforeScrollY}
              contentHeight={beforeContentHeight}
              viewportHeight={beforePane?.clientHeight ?? 0}
              side="left"
              onScroll={handleBeforeScrollbarScroll}
              markers={beforeMarkers}
            />
            <div class="code-container" bind:this={beforePane}>
              <div
                class="lines-wrapper"
                style="transform: translateY(-{scrollController.beforeScrollY}px)"
              >
                {#each beforeLines as line, i}
                  {@const boundary = showRangeMarkers
                    ? getLineBoundary(activeAlignments, 'before', i)
                    : { isStart: false, isEnd: false }}
                  {@const isInHoveredRange = isLineInHoveredRange('before', i)}
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
        </div>
      {/if}

      <!-- Deleted file: before pane shows content -->
      {#if isDeletedFile}
        <div class="diff-pane single-pane-content">
          <div class="pane-header">
            <span class="pane-ref">
              <GitBranch size={12} />
              {diffBaseDisplay}
            </span>
            <span class="pane-path" title={beforePath}>{beforePath ?? 'No file'}</span>
          </div>
          <div class="code-area" onwheel={handleBeforeWheel}>
            <Scrollbar
              scrollY={scrollController.beforeScrollY}
              contentHeight={beforeContentHeight}
              viewportHeight={beforePane?.clientHeight ?? 0}
              side="left"
              onScroll={handleBeforeScrollbarScroll}
              markers={[]}
            />
            <div class="code-container" bind:this={beforePane}>
              <div
                class="lines-wrapper"
                style="transform: translateY(-{scrollController.beforeScrollY}px)"
              >
                {#each beforeLines as line, i}
                  <div class="line">
                    <span class="line-content">
                      {#each getBeforeTokens(i) as token}
                        <span style="color: {token.color}">{token.content}</span>
                      {/each}
                    </span>
                  </div>
                {/each}
              </div>
            </div>
          </div>
        </div>
      {/if}

      <!-- Spine / Divider -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="spine"
        class:dragging={isDraggingDivider}
        onmousedown={handleDividerMouseDown}
        ondblclick={handleDividerDoubleClick}
      >
        <div class="divider-handle"></div>
        <canvas class="spine-connector" bind:this={connectorCanvas}></canvas>
      </div>

      <!-- After pane (two-pane mode or created file) -->
      {#if isTwoPaneMode}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="diff-pane after-pane" style="flex: {1 - paneRatio}">
          <div class="pane-header">
            <span class="pane-ref">
              <GitBranch size={12} />
              {diffHeadDisplay}
            </span>
            <span class="pane-path" title={afterPath}>{afterPath ?? 'No file'}</span>
          </div>
          <div class="code-area" onwheel={handleAfterWheel}>
            <div class="code-container" bind:this={afterPane}>
              <div
                class="lines-wrapper"
                style="transform: translateY(-{scrollController.afterScrollY}px)"
              >
                {#each afterLines as line, i}
                  {@const boundary = showRangeMarkers
                    ? getLineBoundary(activeAlignments, 'after', i)
                    : { isStart: false, isEnd: false }}
                  {@const isInHoveredRange = isLineInHoveredRange('after', i)}
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
            <Scrollbar
              scrollY={scrollController.afterScrollY}
              contentHeight={afterContentHeight}
              viewportHeight={afterPane?.clientHeight ?? 0}
              side="right"
              onScroll={handleAfterScrollbarScroll}
              markers={afterMarkers}
            />
          </div>
        </div>
      {:else if isNewFile}
        <!-- Created file: single after pane -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="diff-pane single-pane-content">
          <div class="pane-header">
            <span class="pane-ref">
              <GitBranch size={12} />
              {diffHeadDisplay}
            </span>
            <span class="pane-path" title={afterPath}>{afterPath ?? 'No file'}</span>
          </div>
          <div class="code-area" onwheel={handleAfterWheel}>
            <div class="code-container" bind:this={afterPane}>
              <div
                class="lines-wrapper"
                style="transform: translateY(-{scrollController.afterScrollY}px)"
              >
                {#each afterLines as line, i}
                  {@const isSelected = isLineSelected('after', i)}
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <div
                    class="line"
                    class:line-selected={isSelected}
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
                    <span class="empty-pane-label">Empty file</span>
                  </div>
                {/if}
              </div>
            </div>
            <Scrollbar
              scrollY={scrollController.afterScrollY}
              contentHeight={afterContentHeight}
              viewportHeight={afterPane?.clientHeight ?? 0}
              side="right"
              onScroll={handleAfterScrollbarScroll}
              markers={[]}
            />
          </div>
        </div>
      {/if}

      <!-- Deleted file: label on right -->
      {#if isDeletedFile}
        <div class="status-label deleted">
          <span class="status-text">Deleted</span>
        </div>
      {/if}
    </div>

    <!-- Range action toolbar (two-pane mode only) -->
    {#if isTwoPaneMode && hoveredRangeIndex !== null && rangeToolbarStyle && commentingOnRange === null}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="range-toolbar"
        style="top: {rangeToolbarStyle.top}px; left: {rangeToolbarStyle.left}px;"
        onmouseleave={handleToolbarMouseLeave}
      >
        <button class="range-btn comment-btn" onclick={handleStartComment} title="Add comment">
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

    <!-- Range comment editor (two-pane mode only) -->
    {#if commentingOnRange !== null && commentEditorStyle}
      {@const existingComments = getCommentsForAlignment(commentingOnRange)}
      {@const existingComment = existingComments[0] ?? null}
      <CommentEditor
        top={commentEditorStyle.top}
        left={commentEditorStyle.left}
        width={commentEditorStyle.width}
        visible={commentEditorStyle.visible}
        {existingComment}
        onSubmit={(content) => {
          if (existingComment) {
            handleCommentEdit(existingComment.id, content);
          } else {
            handleCommentSubmit(content);
          }
          handleCommentCancel();
        }}
        onCancel={handleCommentCancel}
        onDelete={existingComment
          ? () => {
              handleCommentDelete(existingComment.id);
              handleCommentCancel();
            }
          : undefined}
      />
    {/if}

    <!-- Line selection toolbar -->
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
        <button
          class="range-btn comment-btn"
          onclick={handleStartLineComment}
          title="Add comment (Enter)"
        >
          <MessageSquarePlus size={12} />
        </button>
        <button class="range-btn" onclick={clearLineSelection} title="Clear selection (Esc)">
          <X size={12} />
        </button>
      </div>
    {/if}

    <!-- Line comment editor -->
    {#if commentingOnLines && lineCommentEditorStyle}
      {@const existingComment = editingCommentId ? findCommentById(editingCommentId) : null}
      <CommentEditor
        top={lineCommentEditorStyle.top}
        left={lineCommentEditorStyle.left}
        width={lineCommentEditorStyle.width}
        visible={lineCommentEditorStyle.visible}
        {existingComment}
        placeholder="Add a comment on {commentingOnLines.end -
          commentingOnLines.start +
          1} line{commentingOnLines.end !== commentingOnLines.start ? 's' : ''}..."
        onSubmit={(content) => {
          if (existingComment) {
            handleCommentEdit(existingComment.id, content);
            clearLineSelection();
          } else {
            handleLineCommentSubmit(content);
          }
        }}
        onCancel={handleLineCommentCancel}
        onDelete={existingComment
          ? () => {
              handleCommentDelete(existingComment.id);
              clearLineSelection();
            }
          : undefined}
      />
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

  .diff-viewer.loading {
    opacity: 0.6;
    pointer-events: none;
  }

  .loading-overlay {
    position: absolute;
    top: 8px;
    right: 16px;
    z-index: 100;
    padding: 4px 8px;
    background: var(--bg-secondary);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .diff-content {
    display: flex;
    flex: 1;
    overflow: hidden;
    padding-left: 8px;
  }

  .diff-pane {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    min-width: 0;
    position: relative;
    border-radius: 12px;
    background-color: var(--bg-elevated);
  }

  /* Single pane mode */
  .single-pane-content {
    flex: 1;
  }

  /* Status labels for created/deleted files */
  .status-label {
    display: flex;
    align-items: center;
    width: 80px;
    flex-shrink: 0;
  }

  .status-label.created {
    justify-content: flex-end;
    padding-right: 12px;
  }

  .status-label.deleted {
    justify-content: flex-start;
    padding-left: 12px;
  }

  .status-text {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-lg);
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    writing-mode: vertical-rl;
    text-orientation: mixed;
  }

  .status-label.created .status-text {
    transform: rotate(180deg);
    color: var(--status-added);
  }

  .status-label.deleted .status-text {
    color: var(--status-deleted);
  }

  .status-label.reference {
    justify-content: flex-end;
    padding-right: 12px;
  }

  .status-label.reference .status-text {
    transform: rotate(180deg);
    color: var(--text-muted);
  }

  /* Pane header */
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

  /* Spine / Divider */
  .spine {
    width: 24px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background-color: transparent;
    position: relative;
    cursor: col-resize;
  }

  .spine:hover .divider-handle,
  .spine.dragging .divider-handle {
    opacity: 1;
  }

  .divider-handle {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 50%;
    transform: translateX(-50%);
    width: 4px;
    background-color: var(--border-muted);
    border-radius: 2px;
    opacity: 0;
    transition: opacity 0.15s ease;
    pointer-events: none;
    z-index: 10;
  }

  .spine.dragging .divider-handle {
    background-color: var(--accent-primary);
  }

  /* Prevent text selection during drag */
  .diff-viewer:has(.spine.dragging) {
    user-select: none;
  }

  .spine-connector {
    flex: 1;
    width: 100%;
    overflow: visible;
  }

  /* Code area wrapper - contains code-container and scrollbar markers */
  .code-area {
    flex: 1;
    position: relative;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  /* Code container - custom scroll via transform */
  .code-container {
    flex: 1;
    overflow: hidden;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-md);
    line-height: 1.5;
    min-width: 0;
    user-select: none;
    position: relative;
  }

  .lines-wrapper {
    display: inline-block;
    min-width: 100%;
    will-change: transform;
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

  /* Changed line highlight */
  .line.content-changed {
    background-color: var(--diff-changed-bg);
  }

  /* Range boundary markers */
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
    background-color: rgba(128, 128, 128, 0.15);
  }

  /* Line selection highlight */
  .line.line-selected {
    background-color: var(--accent-primary-muted, rgba(59, 130, 246, 0.15));
  }

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
  }

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
</style>
