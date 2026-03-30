import type { Alignment, Comment, SmartDiffAnnotation } from '../types';
import type { Token } from './highlighter';
import { getLineDiffResult } from './inlineDiff.js';
import type { BeforeLineClass, AfterLineClass, CharHighlight } from './inlineDiff.js';

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

export function measureContentWidth(pane: HTMLElement | null): number {
  if (!pane) return 0;
  const linesWrapper = pane.querySelector('.lines-wrapper') as HTMLElement | null;
  return linesWrapper ? linesWrapper.scrollWidth : 0;
}

export function getTokensForLine(tokens: Token[][], index: number): Token[] {
  return tokens[index] || [{ content: '', color: 'inherit' }];
}

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
    width: paneRect.width - paneHorizontalPadding * 2,
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
    width: paneRect.width - paneHorizontalPadding * 2,
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
  alignments: Alignment[],
  beforeLines: string[],
  afterLines: string[]
): BeforeLineClass | AfterLineClass | null {
  const map = side === 'before' ? beforeLineToAlignment : afterLineToAlignment;
  const alignIdx = map.get(lineIndex);
  if (alignIdx === undefined) return null;

  const alignment = alignments[alignIdx];
  const alignBefore = beforeLines.slice(alignment.before.start, alignment.before.end);
  const alignAfter = afterLines.slice(alignment.after.start, alignment.after.end);
  const result = getLineDiffResult(alignBefore, alignAfter);

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
  alignments: Alignment[],
  beforeLines: string[],
  afterLines: string[]
): CharHighlight[] | null {
  const map = side === 'before' ? beforeLineToAlignment : afterLineToAlignment;
  const alignIdx = map.get(lineIndex);
  if (alignIdx === undefined) return null;

  const alignment = alignments[alignIdx];
  const alignBefore = beforeLines.slice(alignment.before.start, alignment.before.end);
  const alignAfter = afterLines.slice(alignment.after.start, alignment.after.end);
  const result = getLineDiffResult(alignBefore, alignAfter);

  const localIdx = side === 'before'
    ? lineIndex - alignment.before.start
    : lineIndex - alignment.after.start;

  const pair = result.modifiedPairs.find(p =>
    side === 'before' ? p.beforeLineIndex === localIdx : p.afterLineIndex === localIdx
  );

  if (!pair) return null;
  return side === 'before' ? pair.beforeHighlights : pair.afterHighlights;
}
