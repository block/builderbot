import type { Alignment, Comment, SmartDiffAnnotation } from '../types';
import type { Token } from './highlighter';
import type { LineDiffCache, BeforeLineClass, AfterLineClass, CharHighlight } from './inlineDiff.js';

export type ChangedAlignmentEntry = { alignment: Alignment; index: number };
export type PaneSide = 'before' | 'after';

export interface LineSelectionState {
  pane: PaneSide;
  anchorLine: number;
  focusLine: number;
}

export interface SelectedLineRange {
  pane: PaneSide;
  start: number;
  end: number;
}
export type CommentPosition = 'above' | 'below';

export interface CommentEditorLayout {
  top: number;
  left: number;
  width: number;
  visible: boolean;
  position: CommentPosition;
}

export interface LineCommentEditorLayout {
  top: number;
  left: number;
  width: number;
  visible: boolean;
  position: CommentPosition;
}

export interface LineSelectionToolbarLayout {
  top: number;
  left: number;
}

export type MarkerType = 'change' | 'comment' | 'annotation';

export interface ScrollbarMarker {
  top: number;
  height: number;
  type: MarkerType;
}

export function buildLineToAlignmentMap(
  changedAlignments: ChangedAlignmentEntry[],
  side: PaneSide
): Map<number, number> {
  const map = new Map<number, number>();
  changedAlignments.forEach(({ alignment }, alignmentIdx) => {
    const span = alignment[side];
    for (let i = span.start; i < span.end; i++) {
      map.set(i, alignmentIdx);
    }
  });
  return map;
}

export function normalizeLineSelection(
  lineSelection: LineSelectionState | null
): SelectedLineRange | null {
  if (!lineSelection) return null;
  const start = Math.min(lineSelection.anchorLine, lineSelection.focusLine);
  const end = Math.max(lineSelection.anchorLine, lineSelection.focusLine);
  return { pane: lineSelection.pane, start, end };
}

export function isLineInChangedAlignment(
  side: PaneSide,
  lineIndex: number,
  beforeLineToAlignment: Map<number, number>,
  afterLineToAlignment: Map<number, number>
): boolean {
  const map = side === 'before' ? beforeLineToAlignment : afterLineToAlignment;
  return map.has(lineIndex);
}

export function isLineSelected(
  pane: PaneSide,
  lineIndex: number,
  selectedLineRange: SelectedLineRange | null
): boolean {
  if (!selectedLineRange || selectedLineRange.pane !== pane) return false;
  return lineIndex >= selectedLineRange.start && lineIndex <= selectedLineRange.end;
}

export function isLineInIndexedRange(
  pane: PaneSide,
  lineIndex: number,
  activeIndex: number | null,
  beforeLineToAlignment: Map<number, number>,
  afterLineToAlignment: Map<number, number>
): boolean {
  if (activeIndex === null) return false;
  const map = pane === 'before' ? beforeLineToAlignment : afterLineToAlignment;
  return map.get(lineIndex) === activeIndex;
}

export function buildBeforeMarkers(
  totalLines: number,
  changedAlignments: ChangedAlignmentEntry[],
  beforeFileAnnotations: SmartDiffAnnotation[]
): ScrollbarMarker[] {
  if (totalLines === 0) return [];

  const changeMarkers = changedAlignments.map(({ alignment }) => {
    const span = alignment.before;
    const startPercent = (span.start / totalLines) * 100;
    const rangeSize = span.end - span.start;
    const heightPercent = Math.max(0.5, (rangeSize / totalLines) * 100);
    return { top: startPercent, height: heightPercent, type: 'change' as const };
  });

  const annotationMarkers = beforeFileAnnotations
    .filter((a) => a.before_span)
    .map((annotation) => {
      const span = annotation.before_span!;
      const startPercent = (span.start / totalLines) * 100;
      const rangeSize = Math.max(1, span.end - span.start);
      const heightPercent = Math.max(0.5, (rangeSize / totalLines) * 100);
      return { top: startPercent, height: heightPercent, type: 'annotation' as const };
    });

  return [...changeMarkers, ...annotationMarkers];
}

export function buildAfterMarkers(
  totalLines: number,
  changedAlignments: ChangedAlignmentEntry[],
  currentFileComments: Comment[],
  currentFileAnnotations: SmartDiffAnnotation[]
): ScrollbarMarker[] {
  if (totalLines === 0) return [];

  const changeMarkers = changedAlignments.map(({ alignment }) => {
    const span = alignment.after;
    const startPercent = (span.start / totalLines) * 100;
    const rangeSize = span.end - span.start;
    const heightPercent = Math.max(0.5, (rangeSize / totalLines) * 100);
    return { top: startPercent, height: heightPercent, type: 'change' as const };
  });

  const commentMarkers = currentFileComments
    .filter((c) => c.span.start !== 0 || c.span.end !== 0)
    .map((comment) => {
      const startPercent = (comment.span.start / totalLines) * 100;
      const rangeSize = Math.max(1, comment.span.end - comment.span.start);
      const heightPercent = Math.max(0.5, (rangeSize / totalLines) * 100);
      return { top: startPercent, height: heightPercent, type: 'comment' as const };
    });

  const annotationMarkers = currentFileAnnotations
    .filter((a) => a.after_span)
    .map((annotation) => {
      const span = annotation.after_span!;
      const startPercent = (span.start / totalLines) * 100;
      const rangeSize = Math.max(1, span.end - span.start);
      const heightPercent = Math.max(0.5, (rangeSize / totalLines) * 100);
      return { top: startPercent, height: heightPercent, type: 'annotation' as const };
    });

  return [...changeMarkers, ...commentMarkers, ...annotationMarkers];
}

export function findCommentById(comments: Comment[], id: string): Comment | null {
  return comments.find((c) => c.id === id) ?? null;
}

export function resolveTrackedComment(
  comments: Comment[],
  activeComment: Comment | null,
  editingCommentId: string | null
): { commentId: string | null; existingComment: Comment | null; missing: boolean } {
  const commentId = activeComment?.id ?? editingCommentId;
  if (!commentId) {
    return { commentId: null, existingComment: null, missing: false };
  }

  const existingComment = findCommentById(comments, commentId);
  return { commentId, existingComment, missing: existingComment === null };
}

export function getCommentsForAlignment(
  alignmentIndex: number,
  changedAlignments: ChangedAlignmentEntry[],
  currentFileComments: Comment[]
): Comment[] {
  const alignmentData = changedAlignments[alignmentIndex];
  if (!alignmentData) return [];

  const { alignment } = alignmentData;
  return currentFileComments.filter(
    (c) => c.span.start < alignment.after.end && c.span.end > alignment.after.start
  );
}

export function measureLineHeight(pane: HTMLElement | null): number {
  if (!pane) return 20;
  const firstLine = pane.querySelector('.line') as HTMLElement | null;
  return firstLine ? firstLine.getBoundingClientRect().height : 20;
}

/** Horizontal padding on `.line-content` (`padding: 0 12px`), in pixels. */
const LINE_CONTENT_PADDING_X = 24;

/**
 * Display-column count of a line, expanding tabs to the next 8-column tab stop
 * (the CSS sets no `tab-size`, so the default 8 applies). Iterates by code
 * point so astral characters count as one. Under a monospace font this is a
 * faithful proxy for which line is widest in pixels.
 */
function displayColumns(line: string): number {
  let columns = 0;
  for (const ch of line) {
    if (ch === '\t') columns += 8 - (columns % 8);
    else columns += 1;
  }
  return columns;
}

/**
 * Width of the widest line in pixels, measured from the source text rather than
 * the rendered DOM.
 *
 * The diff body is virtualized — only a window of lines is mounted — so reading
 * `.lines-wrapper`'s `scrollWidth` only sees the widest *rendered* line and
 * under-clamps horizontal scrolling when the file's longest line is offscreen.
 * Instead, pick the widest line by display-column count and measure just that
 * one line in an offscreen probe span that inherits `.line-content`'s font and
 * `white-space: pre`. One node, so virtualization is preserved, and the result
 * reflects the whole file regardless of scroll position.
 *
 * Returns at least `pane.clientWidth` to keep the `.lines-wrapper`
 * `min-width: 100%` floor.
 */
export function measureContentWidth(lines: string[], pane: HTMLElement | null): number {
  if (!pane) return 0;
  if (lines.length === 0) return 0;

  let widest = '';
  let maxColumns = -1;
  for (const line of lines) {
    const columns = displayColumns(line);
    if (columns > maxColumns) {
      maxColumns = columns;
      widest = line;
    }
  }

  // Probe inside the pane so it inherits the live computed font (font-family,
  // size, and any --code-font-size override), matching the rendered lines.
  const probe = document.createElement('span');
  probe.textContent = widest;
  probe.style.position = 'absolute';
  probe.style.top = '0';
  probe.style.left = '0';
  probe.style.visibility = 'hidden';
  probe.style.whiteSpace = 'pre';
  probe.style.pointerEvents = 'none';
  pane.appendChild(probe);
  const textWidth = probe.getBoundingClientRect().width;
  pane.removeChild(probe);

  return Math.max(Math.ceil(textWidth) + LINE_CONTENT_PADDING_X, pane.clientWidth);
}

export function getTokensForLine(tokens: Token[][], index: number): Token[] {
  return tokens[index] || [{ content: '', color: 'inherit' }];
}

/**
 * Comment text stays readable on very wide panes (e.g. a fullscreen viewer on
 * a wide monitor) by capping the editor at a comfortable prose measure instead
 * of stretching edge to edge with the pane.
 */
export const MAX_COMMENT_EDITOR_WIDTH = 640;

export function decideCommentPositionBySpace(
  spaceBelow: number,
  spaceAbove: number,
  editorHeight: number
): CommentPosition {
  if (spaceBelow >= editorHeight) return 'below';
  if (spaceAbove >= editorHeight) return 'above';
  return spaceBelow >= spaceAbove ? 'below' : 'above';
}

export function buildRangeCommentEditorLayout(
  viewerRect: DOMRect,
  paneRect: DOMRect,
  anchorLineRect: DOMRect,
  position: CommentPosition,
  editorHeight = 120,
  paneHorizontalPadding = 12
): CommentEditorLayout {
  const top =
    position === 'below'
      ? anchorLineRect.bottom - viewerRect.top
      : anchorLineRect.top - viewerRect.top - editorHeight;
  const paneContentTop = paneRect.top - viewerRect.top;
  const paneContentBottom = paneRect.bottom - viewerRect.top;
  const editorBottom = top + editorHeight;
  const visible = editorBottom > paneContentTop && top < paneContentBottom;

  return {
    top,
    left: paneRect.left - viewerRect.left + paneHorizontalPadding,
    width: Math.min(paneRect.width - paneHorizontalPadding * 2, MAX_COMMENT_EDITOR_WIDTH),
    position,
    visible,
  };
}

export function buildLineCommentEditorLayout(
  viewerRect: DOMRect,
  paneRect: DOMRect,
  anchorLineRect: DOMRect,
  position: CommentPosition,
  editorHeight = 120,
  paneHorizontalPadding = 12
): LineCommentEditorLayout {
  const top =
    position === 'below'
      ? anchorLineRect.bottom - viewerRect.top
      : anchorLineRect.top - viewerRect.top - editorHeight;
  const paneContentTop = paneRect.top - viewerRect.top;
  const paneContentBottom = paneRect.bottom - viewerRect.top;
  const editorBottom = top + editorHeight;
  const visible = editorBottom > paneContentTop && top < paneContentBottom;

  return {
    top,
    left: paneRect.left - viewerRect.left + paneHorizontalPadding,
    width: Math.min(paneRect.width - paneHorizontalPadding * 2, MAX_COMMENT_EDITOR_WIDTH),
    visible,
    position,
  };
}

export function resolveLineSelectionToolbarLeft(
  viewerRect: DOMRect,
  lineRect: DOMRect,
  lineContentRect: DOMRect | null,
  currentLeft: number | null,
  recalculateLeft: boolean
): number {
  if (!recalculateLeft && currentLeft !== null) return currentLeft;
  if (lineContentRect) return lineContentRect.left - viewerRect.left;
  return lineRect.left - viewerRect.left;
}

export function buildLineSelectionToolbarLayout(
  viewerRect: DOMRect,
  lineRect: DOMRect,
  left: number
): LineSelectionToolbarLayout {
  return {
    top: lineRect.top - viewerRect.top,
    left,
  };
}

/**
 * Get the line classification for a specific line within a changed alignment.
 * Returns null if the line is not in a changed alignment.
 */
export function getLineClass(
  side: PaneSide,
  lineIndex: number,
  beforeLineToAlignment: Map<number, number>,
  afterLineToAlignment: Map<number, number>,
  changedAlignments: ChangedAlignmentEntry[],
  beforeLines: string[],
  afterLines: string[],
  cache: LineDiffCache
): BeforeLineClass | AfterLineClass | null {
  const map = side === 'before' ? beforeLineToAlignment : afterLineToAlignment;
  const alignIdx = map.get(lineIndex);
  if (alignIdx === undefined) return null;

  const alignment = changedAlignments[alignIdx].alignment;
  const alignBefore = beforeLines.slice(alignment.before.start, alignment.before.end);
  const alignAfter = afterLines.slice(alignment.after.start, alignment.after.end);
  const result = cache.get(alignBefore, alignAfter);

  if (side === 'before') {
    const localIdx = lineIndex - alignment.before.start;
    return result.beforeLines[localIdx];
  } else {
    const localIdx = lineIndex - alignment.after.start;
    return result.afterLines[localIdx];
  }
}

/**
 * Get character highlights for a line if it's a modified line in a changed alignment.
 * Returns null if the line is not modified.
 */
export function getCharHighlights(
  side: PaneSide,
  lineIndex: number,
  beforeLineToAlignment: Map<number, number>,
  afterLineToAlignment: Map<number, number>,
  changedAlignments: ChangedAlignmentEntry[],
  beforeLines: string[],
  afterLines: string[],
  cache: LineDiffCache
): CharHighlight[] | null {
  const map = side === 'before' ? beforeLineToAlignment : afterLineToAlignment;
  const alignIdx = map.get(lineIndex);
  if (alignIdx === undefined) return null;

  const alignment = changedAlignments[alignIdx].alignment;
  const alignBefore = beforeLines.slice(alignment.before.start, alignment.before.end);
  const alignAfter = afterLines.slice(alignment.after.start, alignment.after.end);
  const result = cache.get(alignBefore, alignAfter);

  const localIdx = side === 'before'
    ? lineIndex - alignment.before.start
    : lineIndex - alignment.after.start;

  const pair = result.modifiedPairs.find(p =>
    side === 'before' ? p.beforeLineIndex === localIdx : p.afterLineIndex === localIdx
  );

  if (!pair) return null;
  return side === 'before' ? pair.beforeHighlights : pair.afterHighlights;
}
