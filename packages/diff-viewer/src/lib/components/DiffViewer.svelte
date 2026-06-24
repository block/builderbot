<!--
  DiffViewer.svelte - Unified diff display
  
  Handles three display modes:
  1. Two-pane diff: Side-by-side before/after with synchronized scrolling and spine connectors
  2. Created file: Status label + spine + single after pane (commentable)
  3. Deleted file: Single before pane + spine + status label
  
  The spine is always present - it shows bezier connectors for two-pane diffs,
  and comment highlights for all modes.
  
  Uses custom scroll implementation for frame-perfect sync between panes.
  
  This component is props-driven — it receives diff data, comments, and callbacks
  rather than pulling from global stores.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import type { Snippet } from 'svelte';
  import { MessageSquarePlus, MessageSquare, X, FileText, Code } from 'lucide-svelte';
  import { marked } from 'marked';
  import { sanitize } from '../utils/sanitize';
  import type {
    FileDiff,
    Alignment,
    Comment,
    Span,
    SmartDiffAnnotation,
    CommentActionContext,
  } from '../types';
  import {
    initHighlighter,
    highlightLines,
    detectLanguage,
    prepareLanguage,
    type Token,
  } from '../utils/highlighter';
  import { createScrollController } from '../state/scrollController.svelte';
  import { setupMarkdownScrollSync } from '../utils/markdownScrollSync';
  import {
    ConnectorRendererCanvas,
    type CommentHighlightInfo,
  } from '../utils/connectorRendererCanvas';
  import {
    getLineBoundary,
    getLanguageFromDiff,
    getFilePath,
    isBinaryDiff,
    isImageDiff,
    getTextLines,
  } from '../utils/diffUtils';
  import ImageDiffViewer from './ImageDiffViewer.svelte';
  import type { SearchState, FileSearchResult } from '../state/searchState.svelte';
  import type { SearchMatch, MatchLocation } from '../utils/diffSearch';
  import {
    buildAfterMarkers,
    buildBeforeMarkers,
    buildLineToAlignmentMap,
    findCommentById,
    resolveTrackedComment,
    getCommentsForAlignment,
    getTokensForLine,
    isLineInChangedAlignment as helperIsLineInChangedAlignment,
    isLineInIndexedRange,
    isLineSelected as helperIsLineSelected,
    getLineClass as helperGetLineClass,
    getCharHighlights as helperGetCharHighlights,
    buildLineCommentEditorLayout,
    buildLineSelectionToolbarLayout,
    buildRangeCommentEditorLayout,
    decideCommentPositionBySpace,
    measureContentWidth,
    measureLineHeight,
    normalizeLineSelection,
    resolveLineSelectionToolbarLeft,
  } from '../utils/diffViewerHelpers';
  import { createLineDiffCache } from '../utils/inlineDiff.js';
  import type { BeforeLineClass, AfterLineClass, CharHighlight } from '../utils/inlineDiff.js';
  import { setupDiffKeyboardNav } from '../utils/diffKeyboard';
  import { pathsMatch } from '../utils/diffModalHelpers';
  import {
    DEFAULT_STRUCTURAL_HEADER_MAX_ROWS,
    getHeaderAwareActiveStructuralStack,
    getStructuralDeclarations,
  } from '../utils/structuralHeaders';
  import CommentEditor from './CommentEditor.svelte';
  import AnnotationOverlay from './AnnotationOverlay.svelte';
  import BeforeAnnotationOverlay from './BeforeAnnotationOverlay.svelte';
  import Scrollbar from './Scrollbar.svelte';
  import HorizontalScrollbar from './HorizontalScrollbar.svelte';
  import StructuralHeaderStack from './StructuralHeaderStack.svelte';

  type DiffViewerScrollApi = {
    scrollBy: (side: 'before' | 'after', deltaY: number) => void;
    scrollByX: (side: 'before' | 'after', deltaX: number) => void;
    scrollByXBoth: (deltaX: number) => void;
    canScrollX: (side: 'before' | 'after') => boolean;
  };

  type CommentEditorHandle = {
    ensureSaved: () => Promise<Comment | null>;
    getSaveStatus: () => 'idle' | 'saving' | 'saved' | 'error';
  };

  // ==========================================================================
  // Props
  // ==========================================================================

  interface Props {
    /** The file diff to render (null = no file selected). */
    diff: FileDiff | null;
    /** Comments on this file. */
    comments?: Comment[];
    /** Request to jump to a specific comment in the current file. */
    jumpToComment?: { id: string; token: number } | null;
    /** Request to jump to a specific line in the current file (for search results). */
    jumpToLine?: { lineIndex: number; token: number } | null;
    /** Bumped when syntax theme changes to trigger re-highlight. */
    syntaxThemeVersion?: number;
    /** Whether a new file is loading (show subtle indicator, keep old content). */
    loading?: boolean;
    /** Message shown when no diff content is currently available. */
    emptyMessage?: string;
    /** Whether this is a reference file (not part of the diff). */
    isReferenceFile?: boolean;
    /** Label for the before pane header (e.g. base branch name). */
    beforeLabel?: string;
    /** Label for the after pane header (e.g. head branch name). */
    afterLabel?: string;
    /** AI annotations for the after pane (render-only, not wired to backend). */
    annotations?: SmartDiffAnnotation[];
    /** Whether AI annotation overlays are currently revealed. */
    annotationsRevealed?: boolean;
    /** Search state for highlighting matches in the diff content. */
    searchState?: SearchState;
    /** Host-owned area where clicks may dismiss this viewer's active line selection. */
    clickDismissBoundary?: HTMLElement | null;

    // -- Comment callbacks (all optional; without them commenting is disabled) --
    onAddComment?: (path: string, span: Span, content: string) => Promise<Comment | null>;
    onUpdateComment?: (commentId: string, content: string) => Promise<void>;
    onDeleteComment?: (commentId: string) => Promise<void>;

    /** Host-rendered actions for an existing comment's editor footer. */
    commentActions?: Snippet<[CommentActionContext]>;

    /** Bindable API object exposing scroll control for external callers (e.g. mobile touch scroll). */
    scrollApi?: DiffViewerScrollApi | null;
  }

  let {
    diff,
    comments = [],
    jumpToComment = null,
    jumpToLine = null,
    syntaxThemeVersion = 0,
    loading = false,
    emptyMessage = 'Select a file to view changes',
    isReferenceFile = false,
    beforeLabel = 'before',
    afterLabel = 'after',
    annotations = [],
    annotationsRevealed = false,
    searchState,
    clickDismissBoundary = null,
    onAddComment,
    onUpdateComment,
    onDeleteComment,
    commentActions,
    scrollApi = $bindable(null),
  }: Props = $props();

  // ==========================================================================
  // Inline diff cache (scoped to component instance)
  // ==========================================================================

  const lineDiffCache = createLineDiffCache();

  // ==========================================================================
  // Element refs
  // ==========================================================================

  let beforePane: HTMLDivElement | null = $state(null);
  let afterPane: HTMLDivElement | null = $state(null);
  let connectorCanvas: HTMLCanvasElement | null = $state(null);
  let diffViewerEl: HTMLDivElement | null = $state(null);
  let beforeMarkdownArea: HTMLDivElement | null = $state(null);
  let afterMarkdownArea: HTMLDivElement | null = $state(null);

  /** Tracked width of afterPane for annotation overlays. */
  let afterPaneWidth = $state(0);

  /** Tracked width of beforePane for annotation overlays. */
  let beforePaneWidth = $state(0);

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

  /** Ratio of before pane width (0-1). 0.4 = 40% before, 60% after. */
  let paneRatio = $state(0.4);

  /** Whether user is currently dragging the divider. */
  let isDraggingDivider = $state(false);

  // ==========================================================================
  // Range hover state (for toolbar on changed ranges)
  // ==========================================================================

  let hoveredRangeIndex: number | null = $state(null);
  let rangeToolbarStyle: { top: number; left: number } | null = $state(null);

  // Keyboard navigation focused hunk (set by J/K keys)
  let focusedHunkIndex: number | null = $state(null);

  // ==========================================================================
  // Comment state
  // ==========================================================================

  // Whether commenting is enabled (all three callbacks must be provided)
  let commentingEnabled = $derived(!!onAddComment && !!onUpdateComment && !!onDeleteComment);

  // Range-based commenting (from alignment hover)
  let commentingOnRange: number | null = $state(null);
  let editingRangeCommentId: string | null = $state(null);
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
  let lineCommentPositionPreference: 'above' | 'below' = 'below';
  let editingCommentId: string | null = $state(null);
  let activeLineComment = $state<Comment | null>(null);
  let lineCommentReadOnly = $state(false);
  let lineSelectionToolbarStyle: { top: number; left: number } | null = $state(null);
  let lastHandledJumpToken = $state<number | null>(null);
  let lastHandledJumpLineToken = $state<number | null>(null);
  let commentFocusGeneration = 0;
  let lastAutoScrolledFile: string | null = null;
  let lineCommentEditorRaf: number | null = null;
  let lineSelectionToolbarRaf: number | null = null;
  let rangeCommentEditor: CommentEditorHandle | null = $state(null);
  let lineCommentEditor: CommentEditorHandle | null = $state(null);

  // Markdown preview mode
  let markdownPreview = $state(false);

  // ==========================================================================
  // Derived state
  // ==========================================================================

  // Normalized selection range (start <= end)
  let selectedLineRange = $derived.by(() => {
    return normalizeLineSelection(lineSelection);
  });

  // Alignments from the current diff
  let activeAlignments = $derived(diff?.alignments ?? []);

  // File type detection
  // When both before and after are null, the file wasn't found — treat as no diff
  let isEmptyDiff = $derived(diff !== null && diff.before === null && diff.after === null);
  let isNewFile = $derived(diff !== null && !isEmptyDiff && diff.before === null);
  let isDeletedFile = $derived(diff !== null && !isEmptyDiff && diff.after === null);
  let isTwoPaneMode = $derived(!isNewFile && !isDeletedFile);
  let isBinary = $derived(diff !== null && isBinaryDiff(diff));
  let isImage = $derived(diff !== null && isImageDiff(diff));

  // Extract lines from the diff
  let beforeLines = $derived(diff ? getTextLines(diff, 'before') : []);
  let afterLines = $derived(diff ? getTextLines(diff, 'after') : []);

  // File paths
  let beforePath = $derived(diff?.before?.path ?? null);
  let afterPath = $derived(diff?.after?.path ?? null);
  let currentFilePath = $derived(afterPath ?? beforePath ?? '');

  // Markdown file detection
  let isMarkdownFile = $derived(
    currentFilePath.endsWith('.md') || currentFilePath.endsWith('.markdown')
  );

  // Rendered markdown content
  let beforeMarkdownHtml = $derived.by(() => {
    if (!isMarkdownFile || !markdownPreview) return '';
    const content = beforeLines.join('\n');
    if (!content.trim()) return '';
    const html = marked.parse(content, { async: false }) as string;
    return sanitize(html);
  });

  let afterMarkdownHtml = $derived.by(() => {
    if (!isMarkdownFile || !markdownPreview) return '';
    const content = afterLines.join('\n');
    if (!content.trim()) return '';
    const html = marked.parse(content, { async: false }) as string;
    return sanitize(html);
  });

  // AI annotations for after pane (only informational ones with after_span)
  let currentFileAnnotations = $derived(
    annotations.filter(
      (a) =>
        a.file_path === currentFilePath &&
        a.after_span &&
        (a.category === 'explanation' || a.category === 'context')
    )
  );

  // AI annotations with before_span for the left pane
  let beforeFileAnnotations = $derived(
    annotations.filter(
      (a) =>
        a.file_path === currentFilePath &&
        a.before_span &&
        (a.category === 'explanation' || a.category === 'context')
    )
  );

  let showBeforeAnnotations = $derived(beforeFileAnnotations.length > 0);
  let showAiAnnotations = $derived(currentFileAnnotations.length > 0);

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
    return buildLineToAlignmentMap(changedAlignments, 'before');
  });

  let afterLineToAlignment = $derived.by(() => {
    return buildLineToAlignmentMap(changedAlignments, 'after');
  });

  // Comments for the current file (suffix-match to handle path prefix mismatches)
  let currentFileComments = $derived(
    comments.filter((c) => currentFilePath !== null && pathsMatch(c.path, currentFilePath))
  );
  let activeLineCommentState = $derived(
    resolveTrackedComment(currentFileComments, activeLineComment, editingCommentId)
  );
  let activeRangeCommentState = $derived(
    resolveTrackedComment(currentFileComments, null, editingRangeCommentId)
  );

  // ==========================================================================
  // Custom scroll controller (frame-perfect sync)
  // ==========================================================================

  const scrollController = createScrollController();

  // Expose scroll API for external callers (e.g. mobile touch scroll in DiffModal)
  scrollApi = {
    scrollBy: (side, deltaY) => {
      scrollController.scrollBy(side, deltaY);
      updateScrollPositionedElements();
    },
    scrollByX: (side, deltaX) => {
      scrollController.scrollByX(side, deltaX);
      updateScrollPositionedElements();
    },
    scrollByXBoth: (deltaX) => {
      scrollController.scrollByXBoth(deltaX);
      updateScrollPositionedElements();
    },
    canScrollX: (side) => {
      const dims = scrollController.getDimensions(side);
      return (dims.contentWidth ?? 0) > (dims.viewportWidth ?? 0) + 1;
    },
  };

  function updateScrollPositionedElements() {
    requestAnimationFrame(() => {
      redrawConnectorsImpl();
      updateToolbarPosition();
      updateCommentEditorPosition();
      updateLineSelectionToolbar();
      updateLineCommentEditorPosition();
    });
  }

  let afterStructuralDeclarations = $derived.by(() => {
    if (!afterPath || isDeletedFile || afterLines.length === 0) return [];
    return getStructuralDeclarations(afterPath, afterLines);
  });

  let topVisibleAfterLine = $derived.by(() => {
    const lineHeight = scrollController.getDimensions('after').lineHeight || 20;
    return Math.max(0, Math.floor(scrollController.afterScrollY / lineHeight));
  });

  let activeStructuralStack = $derived.by(() => {
    if (!afterPath || isDeletedFile || (isMarkdownFile && markdownPreview)) return [];
    return getHeaderAwareActiveStructuralStack(
      afterStructuralDeclarations,
      topVisibleAfterLine,
      DEFAULT_STRUCTURAL_HEADER_MAX_ROWS
    );
  });

  // Update scroll controller with active alignments
  $effect(() => {
    const filePath = diff ? getFilePath(diff) : null;
    scrollController.setAlignments(activeAlignments, filePath);
  });

  // Scroll to first diff when file changes
  $effect(() => {
    const filePath = diff ? getFilePath(diff) : null;
    // Only auto-scroll once per file – not when jump tokens or other deps change
    if (!filePath || filePath === lastAutoScrolledFile) return;
    if (changedAlignments.length === 0) return;
    if (!afterPane && !beforePane) return;
    // Skip auto-scroll if a comment or line jump is pending – explicit navigation takes priority
    if (jumpToComment && jumpToComment.token !== lastHandledJumpToken) {
      lastAutoScrolledFile = filePath;
      return;
    }
    if (jumpToLine && jumpToLine.token !== lastHandledJumpLineToken) {
      lastAutoScrolledFile = filePath;
      return;
    }
    lastAutoScrolledFile = filePath;
    // Wait for next frame to ensure dimensions are set
    requestAnimationFrame(() => {
      const firstHunk = changedAlignments[0].alignment;
      // Scroll to first change in the after pane (or before pane for deleted files)
      const side = isDeletedFile ? 'before' : 'after';
      const startRow = side === 'before' ? firstHunk.before.start : firstHunk.after.start;
      scrollController.scrollToRow(startRow, side);
    });
  });

  // Update dimensions when panes are available or content changes
  $effect(() => {
    if (beforePane && beforeLines.length > 0) {
      const lineHeight = measureLineHeight(beforePane);
      const contentWidth = measureContentWidth(beforeLines, beforePane);
      beforeLineHeight = lineHeight || 20;
      beforeViewportHeight = beforePane.clientHeight;
      scrollController.setDimensions('before', {
        viewportHeight: beforePane.clientHeight,
        contentHeight: beforeLines.length * lineHeight,
        lineHeight,
        viewportWidth: beforePane.clientWidth,
        contentWidth,
      });
    }
  });

  $effect(() => {
    if (afterPane && afterLines.length > 0) {
      const lineHeight = measureLineHeight(afterPane);
      const contentWidth = measureContentWidth(afterLines, afterPane);
      afterLineHeight = lineHeight || 20;
      afterViewportHeight = afterPane.clientHeight;
      scrollController.setDimensions('after', {
        viewportHeight: afterPane.clientHeight,
        contentHeight: afterLines.length * lineHeight,
        lineHeight,
        viewportWidth: afterPane.clientWidth,
        contentWidth,
      });
    }
  });

  // Scrollbar marker computation
  let beforeMarkers = $derived.by(() => {
    return buildBeforeMarkers(beforeLines.length, changedAlignments, beforeFileAnnotations);
  });

  let afterMarkers = $derived.by(() => {
    return buildAfterMarkers(
      afterLines.length,
      changedAlignments,
      currentFileComments,
      currentFileAnnotations
    );
  });

  // Content dimensions for scrollbars (state-driven so they update on font changes too)
  let beforeContentHeight = $state(0);
  let afterContentHeight = $state(0);

  // Reactive line/viewport metrics that drive the rendered window (Phase 2
  // virtualization). Kept in sync wherever dimensions are measured below so the
  // window deriveds recompute on resize and font changes, not just on scroll.
  let beforeLineHeight = $state(20);
  let afterLineHeight = $state(20);
  let beforeViewportHeight = $state(0);
  let afterViewportHeight = $state(0);

  // Content width needs to be measured after DOM renders, using state + effect
  let beforeContentWidth = $state(0);
  let afterContentWidth = $state(0);

  // ==========================================================================
  // Rendered window (Phase 2 virtualization)
  // ==========================================================================

  /**
   * Rows rendered above/below the viewport. Generous enough to cover the
   * comment editor's vertical extent (it can render above a multi-line range),
   * so anchor lookups near a viewport edge still resolve to a mounted row.
   */
  const OVERSCAN = 40;

  /**
   * Compute the absolute index range [start, end) to render for a pane given
   * its scroll offset and metrics. Returns absolute indices so every existing
   * `i`-consumer (highlighting, boundaries, class lookups) stays unchanged.
   */
  function computeWindow(
    total: number,
    lineHeight: number,
    scrollY: number,
    viewportHeight: number
  ): { start: number; indices: number[] } {
    const lh = lineHeight || 20;
    const firstVisible = Math.max(0, Math.floor(scrollY / lh));
    const visibleCount = Math.ceil((viewportHeight || 0) / lh);
    const start = Math.max(0, firstVisible - OVERSCAN);
    const end = Math.min(total, firstVisible + visibleCount + OVERSCAN);
    const indices: number[] = [];
    for (let i = start; i < end; i++) indices.push(i);
    return { start, indices };
  }

  let beforeWindow = $derived(
    computeWindow(
      beforeLines.length,
      beforeLineHeight,
      scrollController.beforeScrollY,
      beforeViewportHeight
    )
  );

  let afterWindow = $derived(
    computeWindow(
      afterLines.length,
      afterLineHeight,
      scrollController.afterScrollY,
      afterViewportHeight
    )
  );

  function updateContentWidths() {
    requestAnimationFrame(() => {
      if (beforePane) {
        const lh = measureLineHeight(beforePane) || 20;
        beforeLineHeight = lh;
        beforeViewportHeight = beforePane.clientHeight;
        beforeContentHeight = beforeLines.length * lh;
        beforeContentWidth = measureContentWidth(beforeLines, beforePane);
        scrollController.setDimensions('before', {
          viewportHeight: beforePane.clientHeight,
          contentHeight: beforeContentHeight,
          lineHeight: lh,
          viewportWidth: beforePane.clientWidth,
          contentWidth: beforeContentWidth,
        });
      }
      if (afterPane) {
        const lh = measureLineHeight(afterPane) || 20;
        afterLineHeight = lh;
        afterViewportHeight = afterPane.clientHeight;
        afterContentHeight = afterLines.length * lh;
        afterContentWidth = measureContentWidth(afterLines, afterPane);
        scrollController.setDimensions('after', {
          viewportHeight: afterPane.clientHeight,
          contentHeight: afterContentHeight,
          lineHeight: lh,
          viewportWidth: afterPane.clientWidth,
          contentWidth: afterContentWidth,
        });
      }
    });
  }

  // Re-measure content width when lines change
  $effect(() => {
    const _ = beforeLines.length;
    if (beforePane) updateContentWidths();
  });

  $effect(() => {
    const _ = afterLines.length;
    if (afterPane) updateContentWidths();
  });

  // Re-measure on pane resize (e.g., divider drag) or font CSS variable changes
  $effect(() => {
    if (!beforePane && !afterPane) return;

    const resizeObserver = new ResizeObserver(() => {
      updateContentWidths();
    });

    if (beforePane) resizeObserver.observe(beforePane);
    if (afterPane) resizeObserver.observe(afterPane);

    // Watch for CSS variable changes (e.g. --code-font-size, --font-mono) that
    // affect line metrics without triggering a resize event.
    const styleObserver = new MutationObserver(() => {
      updateContentWidths();
    });
    styleObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['style'],
    });

    return () => {
      resizeObserver.disconnect();
      styleObserver.disconnect();
    };
  });

  // ==========================================================================
  // Effects
  // ==========================================================================

  // Reset UI state on diff change
  $effect(() => {
    if (diff) {
      hoveredRangeIndex = null;
      rangeToolbarStyle = null;
      focusedHunkIndex = null;
      clearLineSelection();
      clearRangeSelection();
    }
  });

  // Keep editor state tied to the latest comments prop. When an opened comment
  // is deleted upstream, close the editor instead of showing a stale or blank one.
  $effect(() => {
    if (activeLineCommentState.missing) {
      clearLineSelection();
    }
  });

  $effect(() => {
    if (activeRangeCommentState.missing) {
      clearRangeSelection();
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
      const t0 = performance.now();
      const beforeCode = beforeLines.join('\n');
      const afterCode = afterLines.join('\n');
      beforeTokens = beforeCode ? highlightLines(beforeCode, language) : [];
      afterTokens = afterCode ? highlightLines(afterCode, language) : [];
      console.info('[diff] syntax highlight', {
        language,
        beforeLines: beforeLines.length,
        afterLines: afterLines.length,
        elapsed: `${(performance.now() - t0).toFixed(1)}ms`,
      });
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

  let previousCanvas: HTMLCanvasElement | null = null;
  $effect(() => {
    if (connectorCanvas && connectorCanvas !== previousCanvas) {
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

    // Don't draw connectors in markdown preview mode
    if (isMarkdownFile && markdownPreview) {
      connectorRenderer.clear();
      return;
    }

    const sourcePane = beforePane ?? afterPane;
    const firstLine = sourcePane.querySelector('.line') as HTMLElement | null;
    const lineHeight = firstLine ? firstLine.getBoundingClientRect().height : 20;

    const canvasRect = connectorCanvas?.getBoundingClientRect();
    const containerRect = afterPane.getBoundingClientRect();
    const verticalOffset = canvasRect ? containerRect.top - canvasRect.top : 0;

    connectorRenderer.render(
      scrollController.beforeScrollY,
      scrollController.afterScrollY,
      lineHeight,
      verticalOffset
    );
  }

  // Redraw connectors when markdown preview mode changes
  $effect(() => {
    const _ = markdownPreview;
    if (diff && connectorCanvas) {
      scheduleConnectorRedraw();
    }
  });

  // Proportional scroll sync for markdown preview mode
  $effect(() => {
    if (!(isMarkdownFile && markdownPreview && beforeMarkdownArea && afterMarkdownArea)) {
      return;
    }
    return setupMarkdownScrollSync(beforeMarkdownArea, afterMarkdownArea);
  });

  // ==========================================================================
  // Token helpers
  // ==========================================================================

  function getBeforeTokens(index: number): Token[] {
    return getTokensForLine(beforeTokens, index);
  }

  function getAfterTokens(index: number): Token[] {
    return getTokensForLine(afterTokens, index);
  }

  // ==========================================================================
  // Search highlighting
  // ==========================================================================

  /** A token segment that may be part of a search match or char-level diff highlight */
  interface HighlightedSegment {
    content: string;
    color: string;
    isMatch: boolean;
    isCurrent: boolean;
    isCharChanged: boolean;
  }

  /**
   * Get search matches for a specific line, marking which is the current match.
   * Returns empty array if no search is active or no matches on this line.
   */
  function getSearchMatchesForLine(
    lineIndex: number,
    side: 'before' | 'after'
  ): Array<MatchLocation & { isCurrent: boolean }> {
    if (!searchState || !diff || side === 'before') return [];

    const filePath = getFilePath(diff);
    if (!filePath) return [];

    const fileResult = searchState.fileResults.get(filePath);
    if (!fileResult) return [];

    // Get all matches on this line
    const lineMatches = fileResult.matches.filter((m) => m.lineIndex === lineIndex);
    if (lineMatches.length === 0) return [];

    // Flatten all results to find global current index
    const flattened: Array<{ filePath: string; match: SearchMatch; globalIndex: number }> = [];
    let globalIdx = 0;
    for (const [path, result] of searchState.fileResults) {
      for (const match of result.matches) {
        flattened.push({ filePath: path, match, globalIndex: globalIdx });
        globalIdx++;
      }
    }

    const currentGlobal = flattened[searchState.currentResultIndex];
    const currentMatch = currentGlobal?.match;

    // Map matches to include isCurrent flag
    return lineMatches.map((m) => {
      const loc = m.right!;
      return {
        startCol: loc.startCol,
        endCol: loc.endCol,
        isCurrent:
          currentGlobal?.filePath === filePath &&
          currentMatch?.lineIndex === lineIndex &&
          currentMatch?.right?.startCol === loc.startCol &&
          currentMatch?.right?.endCol === loc.endCol,
      };
    });
  }

  /**
   * Apply search highlights to syntax tokens by splitting them at match boundaries.
   * This is the core algorithm that segments tokens character-by-character to create
   * highlighted segments that exactly correspond to search matches.
   */
  function applySearchHighlights(
    tokens: Token[],
    matches: Array<MatchLocation & { isCurrent: boolean }>
  ): HighlightedSegment[] {
    if (matches.length === 0) {
      // No matches - return tokens as non-matching segments
      return tokens.map((t) => ({
        content: t.content,
        color: t.color,
        isMatch: false,
        isCurrent: false,
        isCharChanged: false,
      }));
    }

    const segments: HighlightedSegment[] = [];
    let charIndex = 0; // Absolute position in the line

    for (const token of tokens) {
      const tokenStart = charIndex;
      const tokenEnd = charIndex + token.content.length;

      // Find matches that overlap with this token
      const overlappingMatches = matches.filter(
        (m) => m.startCol < tokenEnd && m.endCol > tokenStart
      );

      if (overlappingMatches.length === 0) {
        // Token has no matches - add as single non-match segment
        segments.push({
          content: token.content,
          color: token.color,
          isMatch: false,
          isCurrent: false,
          isCharChanged: false,
        });
      } else {
        // Token has matches - split at match boundaries
        let pos = 0; // Position within token content

        for (const match of overlappingMatches) {
          const matchStart = Math.max(0, match.startCol - tokenStart);
          const matchEnd = Math.min(token.content.length, match.endCol - tokenStart);

          // Add any content before the match
          if (pos < matchStart) {
            segments.push({
              content: token.content.slice(pos, matchStart),
              color: token.color,
              isMatch: false,
              isCurrent: false,
              isCharChanged: false,
            });
          }

          // Add the match itself
          segments.push({
            content: token.content.slice(matchStart, matchEnd),
            color: token.color,
            isMatch: true,
            isCurrent: match.isCurrent,
            isCharChanged: false,
          });

          pos = matchEnd;
        }

        // Add any remaining content after all matches
        if (pos < token.content.length) {
          segments.push({
            content: token.content.slice(pos),
            color: token.color,
            isMatch: false,
            isCurrent: false,
            isCharChanged: false,
          });
        }
      }

      charIndex = tokenEnd;
    }

    return segments;
  }

  /**
   * Apply character-level diff highlights to segments by splitting them at highlight boundaries.
   * Works similarly to applySearchHighlights — walks through segments tracking column position.
   */
  function applyCharHighlights(
    segments: HighlightedSegment[],
    highlights: CharHighlight[]
  ): HighlightedSegment[] {
    if (highlights.length === 0) return segments;

    const result: HighlightedSegment[] = [];
    let charIndex = 0;

    for (const segment of segments) {
      const segStart = charIndex;
      const segEnd = charIndex + segment.content.length;

      // Find highlights that overlap with this segment
      const overlapping = highlights.filter((h) => h.start < segEnd && h.end > segStart);

      if (overlapping.length === 0) {
        result.push(segment);
      } else {
        let pos = 0; // Position within segment content

        for (const hl of overlapping) {
          const hlStart = Math.max(0, hl.start - segStart);
          const hlEnd = Math.min(segment.content.length, hl.end - segStart);

          // Add any content before the highlight
          if (pos < hlStart) {
            result.push({
              ...segment,
              content: segment.content.slice(pos, hlStart),
              isCharChanged: false,
            });
          }

          // Add the highlighted portion
          result.push({
            ...segment,
            content: segment.content.slice(hlStart, hlEnd),
            isCharChanged: true,
          });

          pos = hlEnd;
        }

        // Add any remaining content after all highlights
        if (pos < segment.content.length) {
          result.push({
            ...segment,
            content: segment.content.slice(pos),
            isCharChanged: false,
          });
        }
      }

      charIndex = segEnd;
    }

    return result;
  }

  /**
   * Get highlighted token segments for a line, with search matches and char-level diff applied.
   */
  function getHighlightedTokens(lineIndex: number, side: 'before' | 'after'): HighlightedSegment[] {
    const tokens = side === 'before' ? getBeforeTokens(lineIndex) : getAfterTokens(lineIndex);
    const matches = getSearchMatchesForLine(lineIndex, side);
    let segments = applySearchHighlights(tokens, matches);

    const charHL = getCharHighlightsForLine(side, lineIndex);
    if (charHL && charHL.length > 0) {
      segments = applyCharHighlights(segments, charHL);
    }

    return segments;
  }

  // ==========================================================================
  // Line state helpers
  // ==========================================================================

  function isLineInChangedAlignment(side: 'before' | 'after', lineIndex: number): boolean {
    return helperIsLineInChangedAlignment(
      side,
      lineIndex,
      beforeLineToAlignment,
      afterLineToAlignment
    );
  }

  function getLineClassForLine(
    side: 'before' | 'after',
    lineIndex: number
  ): BeforeLineClass | AfterLineClass | null {
    return helperGetLineClass(
      side,
      lineIndex,
      beforeLineToAlignment,
      afterLineToAlignment,
      changedAlignments,
      beforeLines,
      afterLines,
      lineDiffCache
    );
  }

  function getCharHighlightsForLine(
    side: 'before' | 'after',
    lineIndex: number
  ): CharHighlight[] | null {
    return helperGetCharHighlights(
      side,
      lineIndex,
      beforeLineToAlignment,
      afterLineToAlignment,
      changedAlignments,
      beforeLines,
      afterLines,
      lineDiffCache
    );
  }

  function isLineSelected(pane: 'before' | 'after', lineIndex: number): boolean {
    return helperIsLineSelected(pane, lineIndex, selectedLineRange);
  }

  function isLineInHoveredRange(pane: 'before' | 'after', lineIndex: number): boolean {
    return isLineInIndexedRange(
      pane,
      lineIndex,
      hoveredRangeIndex,
      beforeLineToAlignment,
      afterLineToAlignment
    );
  }

  function isLineInFocusedHunk(pane: 'before' | 'after', lineIndex: number): boolean {
    return isLineInIndexedRange(
      pane,
      lineIndex,
      focusedHunkIndex,
      beforeLineToAlignment,
      afterLineToAlignment
    );
  }

  // ==========================================================================
  // Comment helpers
  // ==========================================================================

  function alignmentHasComments(alignmentIndex: number): boolean {
    return (
      getCommentsForAlignment(alignmentIndex, changedAlignments, currentFileComments).length > 0
    );
  }

  function resolveDisplayRangeFromSpan(span: Span): { start: number; end: number } | null {
    if (afterLines.length === 0) return null;

    const maxLineIndex = afterLines.length - 1;
    const clampLine = (line: number) => Math.max(0, Math.min(maxLineIndex, line));

    const start = clampLine(span.start);
    const end = clampLine(Math.max(span.start, span.end - 1));

    return { start, end };
  }

  // ==========================================================================
  // Scroll handlers (custom scroll via wheel events)
  // ==========================================================================

  function handleWheel(side: 'before' | 'after', e: WheelEvent) {
    e.preventDefault();

    if (e.deltaY !== 0) {
      scrollController.scrollBy(side, e.deltaY);
    }

    const deltaX = e.shiftKey ? e.deltaY : e.deltaX;
    if (deltaX !== 0) {
      scrollController.scrollByXBoth(deltaX);
    }

    requestAnimationFrame(() => {
      redrawConnectorsImpl();
      updateToolbarPosition();
      updateCommentEditorPosition();
      updateLineSelectionToolbar();
      updateLineCommentEditorPosition();
    });
  }

  function handleBeforeWheel(e: WheelEvent) {
    if (!diff) return;
    if (!isTwoPaneMode && !isDeletedFile) return;
    handleWheel('before', e);
  }

  function handleAfterWheel(e: WheelEvent) {
    if (!diff) return;
    handleWheel('after', e);
  }

  function handleBeforeScrollbarScroll(deltaY: number) {
    scrollController.scrollBy('before', deltaY);
    redrawConnectorsImpl();
    updateToolbarPosition();
    updateCommentEditorPosition();
    updateLineSelectionToolbar();
    updateLineCommentEditorPosition();
  }

  function handleAfterScrollbarScroll(deltaY: number) {
    scrollController.scrollBy('after', deltaY);
    redrawConnectorsImpl();
    updateToolbarPosition();
    updateCommentEditorPosition();
    updateLineSelectionToolbar();
    updateLineCommentEditorPosition();
  }

  function handleHorizontalScrollbarScroll(deltaX: number) {
    scrollController.scrollByXBoth(deltaX);
    requestAnimationFrame(() => {
      updateToolbarPosition();
      updateCommentEditorPosition();
      updateLineSelectionToolbar();
      updateLineCommentEditorPosition();
    });
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
    const availableWidth = rect.width - 8 - 24;
    const mouseX = e.clientX - rect.left - 8;

    let ratio = mouseX / availableWidth;
    ratio = Math.max(0.15, Math.min(0.85, ratio));
    paneRatio = ratio;

    redrawConnectorsImpl();
  }

  function handleDividerMouseUp() {
    isDraggingDivider = false;
    document.removeEventListener('mousemove', handleDividerMouseMove);
    document.removeEventListener('mouseup', handleDividerMouseUp);
  }

  function handleDividerDoubleClick() {
    paneRatio = 0.4;
    redrawConnectorsImpl();
  }

  // Redraw connectors when pane ratio changes
  $effect(() => {
    const _ = paneRatio;
    if (diff && connectorCanvas && afterPane) {
      requestAnimationFrame(() => scheduleConnectorRedraw());
    }
  });

  // ==========================================================================
  // Range hover handling
  // ==========================================================================

  // Resolve a line element by its absolute index rather than by NodeList
  // position. Positional lookups (`querySelectorAll('.line')[n]`) only equal the
  // absolute index while every line is in the DOM; identity lookups stay correct
  // once the body is windowed and only a slice of lines is rendered.
  function lineAt(pane: HTMLElement, index: number): HTMLElement | null {
    return pane.querySelector(`.line[data-line-index="${index}"]`);
  }

  // Cap on the number of frames a deferred measure waits for the windowed body
  // to render the rows it needs. A scroll typically renders within one frame;
  // the budget absorbs a slow render pass without spinning indefinitely.
  const WINDOW_RENDER_MAX_FRAMES = 5;

  // Phase 3: scroll → window-render → measure sequencing. Setting a pane's scroll
  // offset recomputes its window and Svelte renders the new rows, but only on a
  // later frame — so an imperative measure on the *next* frame can still find the
  // anchor row unmounted, leaving the editor/toolbar silently unpositioned. Retry
  // `measure` across a bounded number of frames until it reports success: it
  // returns true to stop (positioned, or nothing to do) and false to retry.
  // `trackRaf` lets the caller hold the pending frame id so a newer request can
  // cancel an in-flight wait.
  function measureAfterWindowRender(
    measure: () => boolean,
    trackRaf: (raf: number | null) => void
  ) {
    let frames = 0;
    const tick = () => {
      trackRaf(null);
      if (measure() || frames >= WINDOW_RENDER_MAX_FRAMES) return;
      frames++;
      trackRaf(requestAnimationFrame(tick));
    };
    trackRaf(requestAnimationFrame(tick));
  }

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
    const lineEl = lineAt(afterPane, lineIndex);

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
  // Comment highlight click (from spine)
  // ==========================================================================

  async function focusCommentInViewer(comment: Comment): Promise<boolean> {
    const focusGeneration = ++commentFocusGeneration;

    if (!(await flushAndClearCommentEditors())) return false;
    if (focusGeneration !== commentFocusGeneration) return false;
    if (!afterPane) return false;

    const latestComment = findCommentById(currentFileComments, comment.id) ?? comment;
    const displayRange = resolveDisplayRangeFromSpan(latestComment.span);
    if (!displayRange) return false;
    const { start, end } = displayRange;

    scrollController.scrollToRow(start, 'after');

    lineSelection = { pane: 'after', anchorLine: start, focusLine: end };
    commentingOnLines = { pane: 'after', start, end };
    editingCommentId = latestComment.id;
    activeLineComment = latestComment;
    lineCommentReadOnly = latestComment.author === 'agent';

    // scrollToRow updates pane transforms, but the windowed body only renders the
    // target rows on a later frame — wait for the anchor to mount before deciding
    // the editor's side and positioning it.
    scheduleLineCommentEditorPositioning();
    return true;
  }

  // Resolve the comment editor's side and screen position once the anchor rows are
  // mounted. Returns true to stop the mount-then-measure retry (positioned, or
  // nothing to position), false to retry on a later frame.
  function positionLineCommentEditor(): boolean {
    if (!commentingOnLines) return true;

    const pane = commentingOnLines.pane === 'before' ? beforePane : afterPane;
    if (!pane) return true;

    const firstEl = lineAt(pane, commentingOnLines.start);
    const lastEl = lineAt(pane, commentingOnLines.end);

    // Both ends mounted: full space-based decision (short ranges).
    if (firstEl && lastEl) {
      lineCommentPositionPreference = decideLineCommentPosition();
      updateLineCommentEditorPosition();
      return true;
    }

    // Range taller than the window — both ends can't co-mount in one frame, so
    // decideLineCommentPosition (which measures both) can never run. Anchor to
    // whichever end is rendered. Callers scroll to `start` first, so it lands in
    // the window; position the editor below it (there's space below after the
    // scroll). Pass the resolved row explicitly so the anchor doesn't have to be
    // re-derived from the preference against an unmounted row.
    if (firstEl) {
      lineCommentPositionPreference = 'below';
      updateLineCommentEditorPosition(firstEl);
      return true;
    }
    if (lastEl) {
      lineCommentPositionPreference = 'above';
      updateLineCommentEditorPosition(lastEl);
      return true;
    }

    // Neither end mounted yet: keep retrying for the window render.
    return false;
  }

  function scheduleLineCommentEditorPositioning() {
    if (lineCommentEditorRaf !== null) {
      cancelAnimationFrame(lineCommentEditorRaf);
      lineCommentEditorRaf = null;
    }
    measureAfterWindowRender(positionLineCommentEditor, (raf) => {
      lineCommentEditorRaf = raf;
    });
  }

  async function handleCommentHighlightClick(info: CommentHighlightInfo) {
    if (!afterPane) return;

    const { span, commentId } = info;
    const displayRange = resolveDisplayRangeFromSpan(span);
    if (!displayRange) return;
    const { start, end } = displayRange;

    // Jump to exact comment when available.
    const comment = commentId ? findCommentById(comments, commentId) : null;
    if (comment) {
      await focusCommentInViewer(comment);
      return;
    }

    // Fallback for raw span highlights without a comment id.
    if (!commentingEnabled) {
      scrollController.scrollToRow(start, 'after');
      return;
    }

    const focusGeneration = ++commentFocusGeneration;

    if (!(await flushAndClearCommentEditors())) return;
    if (focusGeneration !== commentFocusGeneration) return;
    if (!afterPane) return;

    const latestDisplayRange = resolveDisplayRangeFromSpan(span);
    if (!latestDisplayRange) return;
    const { start: latestStart, end: latestEnd } = latestDisplayRange;

    scrollController.scrollToRow(latestStart, 'after');

    lineSelection = { pane: 'after', anchorLine: latestStart, focusLine: latestEnd };
    commentingOnLines = { pane: 'after', start: latestStart, end: latestEnd };
    editingCommentId = commentId;
    activeLineComment = comment;
    lineCommentReadOnly = false;
    scheduleLineCommentEditorPositioning();
  }

  // Jump to a comment requested by the sidebar comments list.
  $effect(() => {
    const request = jumpToComment;
    if (!request || !afterPane) return;
    if (lastHandledJumpToken === request.token) return;
    const comment = findCommentById(currentFileComments, request.id);
    if (!comment) return;
    lastHandledJumpToken = request.token;
    void focusCommentInViewer(comment);
  });

  // Jump to a line requested by search results.
  $effect(() => {
    const request = jumpToLine;
    if (!request || !afterPane) return;
    if (lastHandledJumpLineToken === request.token) return;
    lastHandledJumpLineToken = request.token;
    scrollController.scrollToRow(request.lineIndex, 'after');
  });

  // ==========================================================================
  // Range comment handling
  // ==========================================================================

  function handleStartComment() {
    if (hoveredRangeIndex === null || !commentingEnabled) return;
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
    const lastLineEl = lineAt(afterPane, lastLineIndex);
    if (!lastLineEl) return 'below';

    const lastLineRect = lastLineEl.getBoundingClientRect();
    const spaceBelow = paneRect.bottom - lastLineRect.bottom;

    const firstLineEl = lineAt(afterPane, alignment.after.start);
    if (!firstLineEl) return 'below';

    const firstLineRect = firstLineEl.getBoundingClientRect();
    const spaceAbove = firstLineRect.top - paneRect.top;

    return decideCommentPositionBySpace(spaceBelow, spaceAbove, editorHeight);
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
    let anchorLineEl: HTMLElement | null;

    if (commentPositionPreference === 'below') {
      const lastLineIndex = Math.max(alignment.after.start, alignment.after.end - 1);
      anchorLineEl = lineAt(afterPane, lastLineIndex);
      if (!anchorLineEl) {
        commentEditorStyle = null;
        return;
      }
    } else {
      anchorLineEl = lineAt(afterPane, alignment.after.start);
      if (!anchorLineEl) {
        commentEditorStyle = null;
        return;
      }
    }

    commentEditorStyle = buildRangeCommentEditorLayout(
      viewerRect,
      paneRect,
      anchorLineEl.getBoundingClientRect(),
      commentPositionPreference,
      editorHeight
    );
  }

  async function handleCommentSubmit(content: string): Promise<Comment | null> {
    if (commentingOnRange === null || !currentFilePath || !onAddComment) return null;

    const alignmentData = changedAlignments[commentingOnRange];
    if (!alignmentData) return null;

    const { alignment } = alignmentData;
    const span: Span = { start: alignment.after.start, end: alignment.after.end };

    const comment = await onAddComment(currentFilePath, span, content);
    if (comment) {
      editingRangeCommentId = comment.id;
    }
    return comment;
  }

  async function flushRangeCommentEditor(): Promise<boolean> {
    if (!rangeCommentEditor) return true;
    await rangeCommentEditor.ensureSaved();
    return rangeCommentEditor.getSaveStatus() !== 'error';
  }

  async function handleCommentCancel() {
    if (!(await flushRangeCommentEditor())) return;
    clearRangeSelection();
  }

  async function handleCommentEdit(id: string, content: string) {
    await onUpdateComment?.(id, content);
  }

  async function handleCommentDelete(id: string) {
    await onDeleteComment?.(id);
  }

  async function handleRangeCommentDelete(id: string) {
    clearRangeSelection();
    await handleCommentDelete(id);
  }

  async function handleLineCommentDelete(id: string) {
    clearLineSelection();
    await handleCommentDelete(id);
  }

  // ==========================================================================
  // Line selection handling
  // ==========================================================================

  function handleLineMouseDown(pane: 'before' | 'after', lineIndex: number, event: MouseEvent) {
    // Only allow selection on after pane (commentable)
    if (pane === 'before') return;
    if (event.button !== 0) return;
    if (!commentingEnabled) return;

    const previousLineSelection = lineSelection;
    const pendingLineComment = commentingOnLines;
    event.preventDefault();
    window.getSelection()?.removeAllRanges();

    lineSelection = { pane, anchorLine: lineIndex, focusLine: lineIndex };
    isSelecting = true;
    document.addEventListener('mousemove', handleSelectionDragMove);

    if (pendingLineComment) {
      void flushLineCommentEditor().then((canClear) => {
        if (commentingOnLines !== pendingLineComment) return;
        if (!canClear) {
          lineSelection = previousLineSelection;
          isSelecting = false;
          document.removeEventListener('mousemove', handleSelectionDragMove);
          return;
        }
        clearLineCommentEditorState();
      });
      return;
    }

    clearLineCommentEditorState();
  }

  function handleSelectionDragMove(event: MouseEvent) {
    if (!isSelecting || !lineSelection) return;

    const side = lineSelection.pane;
    const pane = side === 'before' ? beforePane : afterPane;
    if (!pane) return;

    // Map the cursor to an absolute line index arithmetically rather than
    // hit-testing rendered `.line` rects: the wrapper is transform-translated by
    // -scrollY over a uniform lineHeight, and once the body is windowed the rows
    // under the cursor may not be mounted.
    const lineHeight = scrollController.getDimensions(side).lineHeight || 20;
    const scrollY =
      side === 'before' ? scrollController.beforeScrollY : scrollController.afterScrollY;
    const total = (side === 'before' ? beforeLines.length : afterLines.length) - 1;
    if (total < 0) return;

    const paneTop = pane.getBoundingClientRect().top;
    const focusLine = Math.min(
      Math.max(Math.floor((event.clientY - paneTop + scrollY) / lineHeight), 0),
      total
    );
    if (lineSelection.focusLine !== focusLine) {
      lineSelection = { ...lineSelection, focusLine };
    }
  }

  function handleLineMouseUp() {
    if (!isSelecting) return;
    isSelecting = false;
    justFinishedSelecting = true;

    document.removeEventListener('mousemove', handleSelectionDragMove);

    if (lineSelection) {
      scheduleLineSelectionToolbar(true);
    }
  }

  // Position the line-selection toolbar once the selection's anchor row is
  // mounted. A drag that ends after auto-scrolling can leave the anchor outside
  // the freshly rendered window for a frame, so wait for it like the editor.
  function scheduleLineSelectionToolbar(recalculateLeft = false) {
    measureAfterWindowRender(
      () => {
        if (!selectedLineRange) return true;
        const pane = selectedLineRange.pane === 'before' ? beforePane : afterPane;
        if (pane && !lineAt(pane, selectedLineRange.start)) return false;
        updateLineSelectionToolbar(recalculateLeft);
        return true;
      },
      (raf) => {
        lineSelectionToolbarRaf = raf;
      }
    );
  }

  function clearLineSelection() {
    lineSelection = null;
    isSelecting = false;
    clearLineCommentEditorState();
  }

  function clearLineCommentEditorState() {
    commentingOnLines = null;
    lineCommentEditorStyle = null;
    lineCommentPositionPreference = 'below';
    editingCommentId = null;
    activeLineComment = null;
    lineCommentReadOnly = false;
  }

  function clearRangeSelection() {
    commentingOnRange = null;
    commentEditorStyle = null;
    editingRangeCommentId = null;
  }

  async function flushLineCommentEditor(): Promise<boolean> {
    if (!lineCommentEditor) return true;
    await lineCommentEditor.ensureSaved();
    return lineCommentEditor.getSaveStatus() !== 'error';
  }

  async function flushAndClearLineSelection() {
    if (!(await flushLineCommentEditor())) return;
    clearLineSelection();
  }

  export async function flushCommentEditors(): Promise<boolean> {
    if (!(await flushRangeCommentEditor())) return false;
    if (!(await flushLineCommentEditor())) return false;
    return true;
  }

  export async function flushAndClearCommentEditors(): Promise<boolean> {
    if (!(await flushCommentEditors())) return false;
    clearRangeSelection();
    clearLineSelection();
    return true;
  }

  export function clearCommentEditors() {
    clearRangeSelection();
    clearLineSelection();
  }

  // Store the initial left position for line selection toolbar
  let lineSelectionToolbarLeft: number | null = $state(null);

  function updateLineSelectionToolbar(recalculateLeft = false) {
    if (!selectedLineRange || !diffViewerEl) {
      lineSelectionToolbarStyle = null;
      lineSelectionToolbarLeft = null;
      return;
    }

    const pane = selectedLineRange.pane === 'before' ? beforePane : afterPane;
    if (!pane) {
      lineSelectionToolbarStyle = null;
      lineSelectionToolbarLeft = null;
      return;
    }

    const lineEl = lineAt(pane, selectedLineRange.start);
    if (!lineEl) {
      lineSelectionToolbarStyle = null;
      lineSelectionToolbarLeft = null;
      return;
    }

    const viewerRect = diffViewerEl.getBoundingClientRect();
    const lineRect = lineEl.getBoundingClientRect();
    const lineContent = lineEl.querySelector('.line-content') as HTMLElement | null;
    const lineContentRect = lineContent?.getBoundingClientRect() ?? null;

    lineSelectionToolbarLeft = resolveLineSelectionToolbarLeft(
      viewerRect,
      lineRect,
      lineContentRect,
      lineSelectionToolbarLeft,
      recalculateLeft
    );

    lineSelectionToolbarStyle = buildLineSelectionToolbarLayout(
      viewerRect,
      lineRect,
      lineSelectionToolbarLeft
    );
  }

  function handleStartLineComment() {
    if (!selectedLineRange || !commentingEnabled) return;
    commentingOnLines = { ...selectedLineRange };
    lineCommentPositionPreference = decideLineCommentPosition();
    editingCommentId = null;
    activeLineComment = null;
    lineCommentReadOnly = false;
    updateLineCommentEditorPosition();
  }

  function decideLineCommentPosition(): 'above' | 'below' {
    if (!commentingOnLines || !diffViewerEl) return 'below';

    const pane = commentingOnLines.pane === 'before' ? beforePane : afterPane;
    if (!pane) return 'below';

    const firstLineEl = lineAt(pane, commentingOnLines.start);
    const lastLineEl = lineAt(pane, commentingOnLines.end);
    if (!firstLineEl || !lastLineEl) return 'below';

    const paneRect = pane.getBoundingClientRect();
    const firstLineRect = firstLineEl.getBoundingClientRect();
    const lastLineRect = lastLineEl.getBoundingClientRect();
    const editorHeight = 120;

    const spaceBelow = paneRect.bottom - lastLineRect.bottom;
    const spaceAbove = firstLineRect.top - paneRect.top;

    return decideCommentPositionBySpace(spaceBelow, spaceAbove, editorHeight);
  }

  function updateLineCommentEditorPosition(anchorOverride?: HTMLElement) {
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
    const editorHeight = 120;
    let anchorLineEl: HTMLElement | null;

    if (anchorOverride) {
      // Tall-range path: the caller resolved the only mounted end; use it directly
      // rather than re-deriving from the preference against an unmounted row.
      anchorLineEl = anchorOverride;
    } else if (lineCommentPositionPreference === 'below') {
      anchorLineEl = lineAt(pane, commentingOnLines.end);
      if (!anchorLineEl) {
        lineCommentEditorStyle = null;
        return;
      }
    } else {
      anchorLineEl = lineAt(pane, commentingOnLines.start);
      if (!anchorLineEl) {
        lineCommentEditorStyle = null;
        return;
      }
    }

    lineCommentEditorStyle = buildLineCommentEditorLayout(
      viewerRect,
      paneRect,
      anchorLineEl.getBoundingClientRect(),
      lineCommentPositionPreference,
      editorHeight
    );
  }

  async function handleLineCommentSubmit(content: string): Promise<Comment | null> {
    if (!commentingOnLines || !currentFilePath || !onAddComment) return null;

    const span: Span = {
      start: commentingOnLines.start,
      end: commentingOnLines.end + 1,
    };

    const comment = await onAddComment(currentFilePath, span, content);
    if (comment) {
      editingCommentId = comment.id;
      activeLineComment = comment;
    }
    return comment;
  }

  async function handleLineCommentCancel() {
    if (!(await flushLineCommentEditor())) return;
    clearLineCommentEditorState();
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

  async function handleGlobalClick(event: MouseEvent) {
    if (justFinishedSelecting) {
      justFinishedSelecting = false;
      return;
    }

    focusedHunkIndex = null;

    const target = event.target as HTMLElement;
    // The comment editor and line selection belong to this viewer. Host apps
    // can extend the dismissal area to adjacent chrome (for example, a sidebar)
    // while body-level dialog portals remain outside that boundary.
    const isInsideDismissArea =
      (diffViewerEl?.contains(target) ?? false) ||
      (clickDismissBoundary?.contains(target) ?? false);
    if (!isInsideDismissArea) {
      return;
    }
    if (
      target.closest('.line-selection-toolbar') ||
      target.closest('.line-comment-editor') ||
      target.closest('.line') ||
      // Sidebar comment clicks should keep the selected comment bubble open.
      target.closest('.comment-item')
    ) {
      return;
    }

    if (commentingOnRange !== null) {
      await handleCommentCancel();
      return;
    }

    if (commentingOnLines) {
      await handleLineCommentCancel();
      return;
    }

    if (lineSelection && !isSelecting) {
      await flushAndClearLineSelection();
    }
  }

  async function handleLineSelectionKeydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return;

    if (event.key === 'Enter' && selectedLineRange && !commentingOnLines) {
      event.preventDefault();
      handleStartLineComment();
      return;
    }

    if (event.key === 'Escape') {
      // Layered dismiss: comment editors first, then line selection
      if (commentingOnLines) {
        event.preventDefault();
        event.stopPropagation();
        await handleLineCommentCancel();
        return;
      }
      if (commentingOnRange !== null) {
        event.preventDefault();
        event.stopPropagation();
        await handleCommentCancel();
        return;
      }
      if (selectedLineRange) {
        event.preventDefault();
        event.stopPropagation();
        await flushAndClearLineSelection();
        return;
      }
    }
  }

  // ==========================================================================
  // Copy handling
  // ==========================================================================

  function handleCopy(event: ClipboardEvent) {
    // Don't intercept copy from form elements (e.g. comment textareas)
    const target = document.activeElement;
    if (target instanceof HTMLTextAreaElement || target instanceof HTMLInputElement) return;

    if (selectedLineRange) {
      event.preventDefault();
      // Reconstruct from the source-line array by index rather than the DOM:
      // with the body windowed, rows outside the rendered slice aren't mounted,
      // so a DOM-based read would silently drop lines from a tall selection.
      const sourceLines = selectedLineRange.pane === 'before' ? beforeLines : afterLines;
      const lines: string[] = [];

      for (let i = selectedLineRange.start; i <= selectedLineRange.end; i++) {
        if (i >= 0 && i < sourceLines.length) {
          lines.push(sourceLines[i]);
        }
      }

      if (lines.length > 0) {
        event.clipboardData?.setData('text/plain', lines.join('\n'));
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
      event.clipboardData?.setData('text/plain', lines.join('\n'));
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
      startCommentOnHunk: (hunkIndex) => {
        if (!commentingEnabled) return;
        commentingOnRange = hunkIndex;
        commentPositionPreference = decideCommentPosition();
        updateCommentEditorPosition();
      },
      onHunkFocus: (hunkIndex) => {
        focusedHunkIndex = hunkIndex;
      },
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
      document.removeEventListener('mousemove', handleSelectionDragMove);
      document.removeEventListener('mousemove', handleDividerMouseMove);
      document.removeEventListener('mouseup', handleDividerMouseUp);
      if (lineCommentEditorRaf !== null) {
        cancelAnimationFrame(lineCommentEditorRaf);
        lineCommentEditorRaf = null;
      }
      if (lineSelectionToolbarRaf !== null) {
        cancelAnimationFrame(lineSelectionToolbarRaf);
        lineSelectionToolbarRaf = null;
      }
      if (connectorRenderer) {
        connectorRenderer.destroy();
        connectorRenderer = null;
      }
    };
  });

  // Track pane widths for annotation overlays
  $effect(() => {
    if (!afterPane) return;
    afterPaneWidth = afterPane.clientWidth;
    const resizeObserver = new ResizeObserver((entries) => {
      for (const entry of entries) {
        afterPaneWidth = entry.contentRect.width;
      }
    });
    resizeObserver.observe(afterPane);
    return () => resizeObserver.disconnect();
  });

  $effect(() => {
    if (!beforePane) return;
    beforePaneWidth = beforePane.clientWidth;
    const resizeObserver = new ResizeObserver((entries) => {
      for (const entry of entries) {
        beforePaneWidth = entry.contentRect.width;
      }
    });
    resizeObserver.observe(beforePane);
    return () => resizeObserver.disconnect();
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
      <p>{emptyMessage}</p>
    </div>
  {:else if isEmptyDiff}
    <div class="empty-state">
      <p>File not found in this diff</p>
    </div>
  {:else if isImage}
    <ImageDiffViewer
      beforeSrc={diff.before?.content.type === 'ImageBase64'
        ? `data:${diff.before.content.mimeType};base64,${diff.before.content.data}`
        : null}
      afterSrc={diff.after?.content.type === 'ImageBase64'
        ? `data:${diff.after.content.mimeType};base64,${diff.after.content.data}`
        : null}
    />
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
            <span class="pane-label">{beforeLabel}</span>
            <span class="pane-path" title={beforePath}>{beforePath ?? 'No file'}</span>
            {#if isMarkdownFile}
              <button
                class="markdown-toggle"
                onclick={() => (markdownPreview = !markdownPreview)}
                title={markdownPreview ? 'Show code' : 'Preview markdown'}
              >
                {#if markdownPreview}<Code size={14} />{:else}<FileText size={14} />{/if}
              </button>
            {/if}
          </div>
          <div
            class="code-area"
            class:markdown-mode={isMarkdownFile && markdownPreview}
            onwheel={isMarkdownFile && markdownPreview ? undefined : handleBeforeWheel}
            bind:this={beforeMarkdownArea}
          >
            {#if isMarkdownFile && markdownPreview}
              <div class="markdown-preview-container">
                <div class="markdown-body">
                  {@html beforeMarkdownHtml}
                </div>
              </div>
            {:else}
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
                  style="transform: translate(-{scrollController.beforeScrollX}px, -{scrollController.beforeScrollY}px)"
                >
                  <div
                    class="line-spacer"
                    style="height: {beforeWindow.start * beforeLineHeight}px"
                  ></div>
                  {#each beforeWindow.indices as i (i)}
                    {@const boundary = showRangeMarkers
                      ? getLineBoundary(activeAlignments, 'before', i)
                      : { isStart: false, isEnd: false }}
                    {@const isInHoveredRange = isLineInHoveredRange('before', i)}
                    {@const isInFocusedHunk = isLineInFocusedHunk('before', i)}
                    {@const lineClass = showRangeMarkers ? getLineClassForLine('before', i) : null}
                    {@const isChanged = lineClass !== null && lineClass !== 'unchanged'}
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <div
                      class="line"
                      data-line-index={i}
                      class:range-start={boundary.isStart}
                      class:range-end={boundary.isEnd}
                      class:range-hovered={isInHoveredRange}
                      class:range-focused={isInFocusedHunk}
                      class:content-changed={isChanged && lineClass !== 'modified'}
                      class:diff-modified={lineClass === 'modified'}
                      onmouseenter={() => handleLineMouseEnter('before', i)}
                      onmouseleave={handleLineMouseLeave}
                    >
                      <span class="line-content">
                        {#each getHighlightedTokens(i, 'before') as segment}
                          <span
                            style="color: {segment.color}"
                            class:search-match={segment.isMatch && !segment.isCurrent}
                            class:search-current={segment.isCurrent}
                            class:char-changed={segment.isCharChanged}
                          >
                            {segment.content}
                          </span>
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
                <!-- AI blur overlay for before pane -->
                {#if showBeforeAnnotations && annotationsRevealed}
                  {@const lineHeight = scrollController.getDimensions('before').lineHeight || 20}
                  <div class="ai-blur-overlay">
                    {#each beforeFileAnnotations as annotation}
                      {#if annotation.before_span}
                        <BeforeAnnotationOverlay
                          {annotation}
                          top={annotation.before_span.start * lineHeight -
                            scrollController.beforeScrollY}
                          height={(annotation.before_span.end - annotation.before_span.start) *
                            lineHeight}
                          revealed={true}
                          containerWidth={beforePaneWidth}
                        />
                      {/if}
                    {/each}
                  </div>
                {/if}
              </div>
              <HorizontalScrollbar
                scrollX={scrollController.beforeScrollX}
                contentWidth={beforeContentWidth}
                viewportWidth={beforePane?.clientWidth ?? 0}
                onScroll={handleHorizontalScrollbarScroll}
              />
            {/if}
          </div>
        </div>
      {/if}

      <!-- Deleted file: before pane shows content -->
      {#if isDeletedFile}
        <div class="diff-pane single-pane-content">
          <div class="pane-header">
            <span class="pane-label">{beforeLabel}</span>
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
                style="transform: translate(-{scrollController.beforeScrollX}px, -{scrollController.beforeScrollY}px)"
              >
                <div
                  class="line-spacer"
                  style="height: {beforeWindow.start * beforeLineHeight}px"
                ></div>
                {#each beforeWindow.indices as i (i)}
                  <div class="line" data-line-index={i}>
                    <span class="line-content">
                      {#each getHighlightedTokens(i, 'before') as segment}
                        <span
                          style="color: {segment.color}"
                          class:search-match={segment.isMatch && !segment.isCurrent}
                          class:search-current={segment.isCurrent}
                          class:char-changed={segment.isCharChanged}
                        >
                          {segment.content}
                        </span>
                      {/each}
                    </span>
                  </div>
                {/each}
              </div>
            </div>
            <HorizontalScrollbar
              scrollX={scrollController.beforeScrollX}
              contentWidth={beforeContentWidth}
              viewportWidth={beforePane?.clientWidth ?? 0}
              onScroll={handleHorizontalScrollbarScroll}
            />
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
        <canvas
          class="spine-connector"
          class:hidden={isMarkdownFile && markdownPreview}
          bind:this={connectorCanvas}
        ></canvas>
      </div>

      <!-- After pane (two-pane mode or created file) -->
      {#if isTwoPaneMode}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="diff-pane after-pane" style="flex: {1 - paneRatio}">
          <div class="pane-header">
            <span class="pane-label">{afterLabel}</span>
            <span class="pane-path" title={afterPath}>{afterPath ?? 'No file'}</span>
            {#if isMarkdownFile}
              <button
                class="markdown-toggle"
                onclick={() => (markdownPreview = !markdownPreview)}
                title={markdownPreview ? 'Show code' : 'Preview markdown'}
              >
                {#if markdownPreview}<Code size={14} />{:else}<FileText size={14} />{/if}
              </button>
            {/if}
          </div>
          <div
            class="code-area"
            class:markdown-mode={isMarkdownFile && markdownPreview}
            onwheel={isMarkdownFile && markdownPreview ? undefined : handleAfterWheel}
            bind:this={afterMarkdownArea}
          >
            {#if isMarkdownFile && markdownPreview}
              <div class="markdown-preview-container">
                <div class="markdown-body">
                  {@html afterMarkdownHtml}
                </div>
              </div>
            {:else}
              <StructuralHeaderStack
                stack={activeStructuralStack}
                maxRows={DEFAULT_STRUCTURAL_HEADER_MAX_ROWS}
              />
              <div class="code-container" bind:this={afterPane}>
                <div
                  class="lines-wrapper"
                  style="transform: translate(-{scrollController.afterScrollX}px, -{scrollController.afterScrollY}px)"
                >
                  <div
                    class="line-spacer"
                    style="height: {afterWindow.start * afterLineHeight}px"
                  ></div>
                  {#each afterWindow.indices as i (i)}
                    {@const boundary = showRangeMarkers
                      ? getLineBoundary(activeAlignments, 'after', i)
                      : { isStart: false, isEnd: false }}
                    {@const isInHoveredRange = isLineInHoveredRange('after', i)}
                    {@const isInFocusedHunk = isLineInFocusedHunk('after', i)}
                    {@const lineClass = showRangeMarkers ? getLineClassForLine('after', i) : null}
                    {@const isChanged = lineClass !== null && lineClass !== 'unchanged'}
                    {@const isSelected = isLineSelected('after', i)}
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <div
                      class="line"
                      data-line-index={i}
                      class:range-start={boundary.isStart}
                      class:range-end={boundary.isEnd}
                      class:range-hovered={isInHoveredRange}
                      class:range-focused={isInFocusedHunk}
                      class:content-changed={isChanged && lineClass !== 'modified'}
                      class:diff-modified={lineClass === 'modified'}
                      class:line-selected={isSelected}
                      onmouseenter={() => handleLineMouseEnter('after', i)}
                      onmouseleave={handleLineMouseLeave}
                      onmousedown={(e) => handleLineMouseDown('after', i, e)}
                    >
                      <span class="line-content">
                        {#each getHighlightedTokens(i, 'after') as segment}
                          <span
                            style="color: {segment.color}"
                            class:search-match={segment.isMatch && !segment.isCurrent}
                            class:search-current={segment.isCurrent}
                            class:char-changed={segment.isCharChanged}
                          >
                            {segment.content}
                          </span>
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
                <!-- AI blur overlay for after pane -->
                {#if showAiAnnotations && annotationsRevealed}
                  {@const lineHeight = scrollController.getDimensions('after').lineHeight || 20}
                  <div class="ai-blur-overlay">
                    {#each currentFileAnnotations as annotation}
                      {#if annotation.after_span}
                        <AnnotationOverlay
                          {annotation}
                          top={annotation.after_span.start * lineHeight -
                            scrollController.afterScrollY}
                          height={(annotation.after_span.end - annotation.after_span.start) *
                            lineHeight}
                          revealed={true}
                          containerWidth={afterPaneWidth}
                        />
                      {/if}
                    {/each}
                  </div>
                {/if}
              </div>
              <Scrollbar
                scrollY={scrollController.afterScrollY}
                contentHeight={afterContentHeight}
                viewportHeight={afterPane?.clientHeight ?? 0}
                side="right"
                onScroll={handleAfterScrollbarScroll}
                markers={afterMarkers}
              />
              <HorizontalScrollbar
                scrollX={scrollController.afterScrollX}
                contentWidth={afterContentWidth}
                viewportWidth={afterPane?.clientWidth ?? 0}
                onScroll={handleHorizontalScrollbarScroll}
              />
            {/if}
          </div>
        </div>
      {:else if isNewFile}
        <!-- Created file: single after pane -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="diff-pane single-pane-content">
          <div class="pane-header">
            <span class="pane-label">{afterLabel}</span>
            <span class="pane-path" title={afterPath}>{afterPath ?? 'No file'}</span>
          </div>
          <div class="code-area" onwheel={handleAfterWheel}>
            <StructuralHeaderStack
              stack={activeStructuralStack}
              maxRows={DEFAULT_STRUCTURAL_HEADER_MAX_ROWS}
            />
            <div class="code-container" bind:this={afterPane}>
              <div
                class="lines-wrapper"
                style="transform: translate(-{scrollController.afterScrollX}px, -{scrollController.afterScrollY}px)"
              >
                <div
                  class="line-spacer"
                  style="height: {afterWindow.start * afterLineHeight}px"
                ></div>
                {#each afterWindow.indices as i (i)}
                  {@const isSelected = isLineSelected('after', i)}
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <div
                    class="line"
                    data-line-index={i}
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
              <!-- AI blur overlay for after pane (new file mode) -->
              {#if showAiAnnotations && annotationsRevealed}
                {@const lineHeight = scrollController.getDimensions('after').lineHeight || 20}
                <div class="ai-blur-overlay">
                  {#each currentFileAnnotations as annotation}
                    {#if annotation.after_span}
                      <AnnotationOverlay
                        {annotation}
                        top={annotation.after_span.start * lineHeight -
                          scrollController.afterScrollY}
                        height={(annotation.after_span.end - annotation.after_span.start) *
                          lineHeight}
                        revealed={true}
                        containerWidth={afterPaneWidth}
                      />
                    {/if}
                  {/each}
                </div>
              {/if}
            </div>
            <Scrollbar
              scrollY={scrollController.afterScrollY}
              contentHeight={afterContentHeight}
              viewportHeight={afterPane?.clientHeight ?? 0}
              side="right"
              onScroll={handleAfterScrollbarScroll}
              markers={[]}
            />
            <HorizontalScrollbar
              scrollX={scrollController.afterScrollX}
              contentWidth={afterContentWidth}
              viewportWidth={afterPane?.clientWidth ?? 0}
              onScroll={handleHorizontalScrollbarScroll}
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

    <!-- Range action toolbar (two-pane mode only, when commenting is enabled) -->
    {#if isTwoPaneMode && commentingEnabled && hoveredRangeIndex !== null && rangeToolbarStyle && commentingOnRange === null}
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
      </div>
    {/if}

    <!-- Range comment editor (two-pane mode only) -->
    {#if commentingOnRange !== null && commentEditorStyle && !activeRangeCommentState.missing}
      {@const existingComment = activeRangeCommentState.existingComment}
      <CommentEditor
        bind:this={rangeCommentEditor}
        top={commentEditorStyle.top}
        left={commentEditorStyle.left}
        width={commentEditorStyle.width}
        visible={commentEditorStyle.visible}
        existingComment={existingComment ?? null}
        onSave={(commentId, content) => {
          if (commentId) {
            return handleCommentEdit(commentId, content);
          }
          return handleCommentSubmit(content);
        }}
        onClose={handleCommentCancel}
        onDelete={existingComment ? () => handleRangeCommentDelete(existingComment.id) : undefined}
        {commentActions}
      />
    {/if}

    <!-- Line selection toolbar -->
    {#if commentingEnabled && selectedLineRange && lineSelectionToolbarStyle && !commentingOnLines}
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
    {#if commentingOnLines && lineCommentEditorStyle && !activeLineCommentState.missing}
      {@const existingComment = activeLineCommentState.existingComment}
      <CommentEditor
        bind:this={lineCommentEditor}
        top={lineCommentEditorStyle.top}
        left={lineCommentEditorStyle.left}
        width={lineCommentEditorStyle.width}
        visible={lineCommentEditorStyle.visible}
        {existingComment}
        readOnly={lineCommentReadOnly}
        placeholder="Add a comment on {commentingOnLines.end -
          commentingOnLines.start +
          1} line{commentingOnLines.end !== commentingOnLines.start ? 's' : ''}..."
        onSave={(commentId, content) => {
          if (commentId) {
            return handleCommentEdit(commentId, content);
          }
          return handleLineCommentSubmit(content);
        }}
        onClose={handleLineCommentCancel}
        onDelete={existingComment ? () => handleLineCommentDelete(existingComment.id) : undefined}
        {commentActions}
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
    background-color: var(--bg-primary);
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
    font-family: var(--font-mono, 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace);
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
    border-bottom: none;
  }

  .pane-label {
    font-family: var(--font-mono, 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace);
    font-size: var(--size-xs);
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .pane-path {
    font-family: var(--font-mono, 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace);
    font-size: var(--size-sm);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .markdown-toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    margin-left: auto;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .markdown-toggle:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .code-area.markdown-mode {
    overflow-y: auto;
  }

  .markdown-preview-container {
    padding: 16px 20px;
  }

  .markdown-body {
    color: var(--text-primary);
    font-size: var(--size-sm);
    line-height: 1.6;
  }

  .markdown-body :global(h1),
  .markdown-body :global(h2),
  .markdown-body :global(h3),
  .markdown-body :global(h4),
  .markdown-body :global(h5),
  .markdown-body :global(h6) {
    margin-top: 1.5em;
    margin-bottom: 0.5em;
    font-weight: 600;
    line-height: 1.3;
  }

  .markdown-body :global(h1) {
    font-size: 1.75em;
  }
  .markdown-body :global(h2) {
    font-size: 1.5em;
  }
  .markdown-body :global(h3) {
    font-size: 1.25em;
  }
  .markdown-body :global(h4) {
    font-size: 1.1em;
  }

  .markdown-body :global(p) {
    margin: 0.75em 0;
  }

  .markdown-body :global(code) {
    padding: 0.2em 0.4em;
    background-color: var(--bg-primary);
    border-radius: 4px;
    font-family: var(--font-mono, 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace);
    font-size: 0.9em;
  }

  .markdown-body :global(pre) {
    margin: 1em 0;
    padding: 12px 16px;
    background-color: var(--bg-primary);
    border-radius: 6px;
    overflow-x: auto;
  }

  .markdown-body :global(pre code) {
    padding: 0;
    background: none;
  }

  .markdown-body :global(ul),
  .markdown-body :global(ol) {
    margin: 0.75em 0;
    padding-left: 1.5em;
  }

  .markdown-body :global(li) {
    margin: 0.25em 0;
  }

  .markdown-body :global(blockquote) {
    margin: 1em 0;
    padding: 0.5em 1em;
    border-left: 4px solid var(--border-muted);
    color: var(--text-muted);
    background-color: var(--bg-primary);
  }

  .markdown-body :global(a) {
    color: var(--text-link);
    text-decoration: none;
  }

  .markdown-body :global(a:hover) {
    text-decoration: underline;
  }

  .markdown-body :global(table) {
    width: 100%;
    border-collapse: collapse;
    margin: 1em 0;
  }

  .markdown-body :global(th),
  .markdown-body :global(td) {
    padding: 8px 12px;
    border: 1px solid var(--border-muted);
    text-align: left;
  }

  .markdown-body :global(th) {
    background-color: var(--bg-primary);
    font-weight: 600;
  }

  .markdown-body :global(hr) {
    border: none;
    border-top: 1px solid var(--border-muted);
    margin: 1.5em 0;
  }

  .markdown-body :global(img) {
    max-width: 100%;
    height: auto;
  }

  /* Spine / Divider */
  .spine {
    width: 16px;
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
    background-color: var(--ui-accent);
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

  .spine-connector.hidden {
    visibility: hidden;
  }

  /* Code area wrapper */
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
    font-family: var(--font-mono, 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace);
    font-size: var(--code-font-size, var(--size-md));
    line-height: 1.5;
    min-width: 0;
    user-select: none;
    position: relative;
  }

  .lines-wrapper {
    display: block;
    will-change: transform;
    position: relative;
    width: max-content;
    min-width: 100%;
  }

  .line {
    display: flex;
    min-height: calc(var(--size-md) * 1.5);
    position: relative;
  }

  /* Top spacer reserving the height of unrendered rows above the window. */
  .line-content {
    flex: 1;
    padding: 0 12px;
    white-space: pre;
  }

  /* Changed line highlight — neutral fallback */
  .line.content-changed {
    background-color: var(--diff-changed-bg);
  }

  /* Per-pane diff colors: deletions in before, additions in after */
  .before-pane .line.content-changed {
    background-color: var(--diff-removed-bg);
  }

  .after-pane .line.content-changed {
    background-color: var(--diff-added-bg);
  }

  .line.diff-modified {
    background-color: var(--diff-modified-bg);
  }

  .char-changed {
    background-color: var(--diff-modified-inline-bg);
    border-radius: 2px;
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

  /* Keyboard-navigated focused hunk */
  .line.range-focused {
    background-color: color-mix(in srgb, var(--ui-accent) 12%, transparent);
    box-shadow: inset 3px 0 0 var(--ui-accent);
  }

  .line.range-focused.content-changed {
    background-color: color-mix(in srgb, var(--ui-accent) 18%, transparent);
  }

  /* Line selection highlight */
  .line.line-selected {
    background-color: color-mix(in srgb, var(--ui-accent) 15%, transparent);
  }

  .line.line-selected.content-changed,
  .line.line-selected.range-hovered {
    background-color: color-mix(in srgb, var(--ui-accent) 15%, transparent);
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
    border: none;
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

  .range-btn.comment-btn:hover {
    color: var(--ui-accent);
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
    border: none;
    border-radius: 4px 4px 0 0;
    padding: 0 4px;
  }

  .selection-info {
    font-size: var(--size-xs);
    color: var(--text-muted);
    padding: 4px 4px;
    white-space: nowrap;
  }

  /* Full-pane AI blur overlay */
  .ai-blur-overlay {
    position: absolute;
    inset: 0;
    z-index: 10;
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);
    background: rgba(var(--ui-accent-rgb, 59, 130, 246), 0.08);
    pointer-events: none;
  }

  /* Search result highlighting */
  .search-match {
    background-color: rgba(250, 200, 50, 0.35);
    border-radius: 2px;
  }

  .search-current {
    background-color: rgba(255, 150, 50, 0.5);
    border-radius: 2px;
  }

  @media (prefers-color-scheme: light) {
    .search-match {
      background-color: rgba(250, 200, 50, 0.5);
    }

    .search-current {
      background-color: rgba(255, 150, 50, 0.6);
    }
  }
</style>
