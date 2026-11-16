/**
 * The board's sideways overflow: which edges have columns beyond them, and how
 * far a chevron press scrolls. Kept free of the DOM so the rules are
 * unit-testable; BoardScrollEdges feeds it the scroller's numbers and rects.
 */

/** How wide each edge's fade is, in px. The chevron reveals columns past it. */
export const SCROLL_FADE_WIDTH = 48;

/** Slack for fractional scroll positions under zoom. */
const EPSILON = 1;

export interface ScrollMetrics {
  scrollLeft: number;
  scrollWidth: number;
  clientWidth: number;
}

export interface ScrollEdges {
  /** Columns are scrolled out past the left edge. */
  start: boolean;
  /** Columns run past the right edge. */
  end: boolean;
}

export function overflowEdges({
  scrollLeft,
  scrollWidth,
  clientWidth,
}: ScrollMetrics): ScrollEdges {
  return {
    start: scrollLeft > EPSILON,
    end: scrollLeft + clientWidth < scrollWidth - EPSILON,
  };
}

export interface Span {
  left: number;
  right: number;
}

/**
 * How far to scroll (negative is left) to bring the next clipped column on
 * that side fully into `view`: the first column past its right edge lines up
 * with that edge, or the last one before its left edge with that edge. `view`
 * is the part of the scroller the fades don't cover. With no clipped column
 * (columns wider than the view, say), it scrolls by the view's width.
 */
export function revealStep(direction: 1 | -1, view: Span, columns: readonly Span[]): number {
  if (direction > 0) {
    const next = columns.find((column) => column.right > view.right + EPSILON);
    return next ? next.right - view.right : view.right - view.left;
  }
  for (let i = columns.length - 1; i >= 0; i--) {
    if (columns[i].left < view.left - EPSILON) return columns[i].left - view.left;
  }
  return view.left - view.right;
}

/**
 * How far to scroll (negative is left) to bring `column` fully into `view`,
 * or 0 when it already is. A column wider than the view lines its left edge
 * up with the view's.
 */
export function revealOffset(view: Span, column: Span): number {
  const wider = column.right - column.left > view.right - view.left;
  if (wider || column.left < view.left - EPSILON) return column.left - view.left;
  if (column.right > view.right + EPSILON) return column.right - view.right;
  return 0;
}

/** How close to a side of the board, in px, a dragged card starts it scrolling. */
export const DRAG_SCROLL_EDGE = 64;

/** The fastest a dragged card scrolls the board, in px per second. */
export const DRAG_SCROLL_MAX_SPEED = 900;

/**
 * How fast (px per second, negative is left) a card dragged to `x` scrolls
 * the board sideways: nothing in the middle of `view`, then faster the
 * nearer the pointer is to a side, and flat out at or past it. A view too
 * narrow for two edge bands splits down the middle.
 */
export function edgeScrollVelocity(x: number, view: Span): number {
  const band = Math.min(DRAG_SCROLL_EDGE, (view.right - view.left) / 2);
  if (band <= 0) return 0;
  const intoStart = view.left + band - x;
  const intoEnd = x - (view.right - band);
  if (intoStart > 0) return -DRAG_SCROLL_MAX_SPEED * Math.min(1, intoStart / band);
  if (intoEnd > 0) return DRAG_SCROLL_MAX_SPEED * Math.min(1, intoEnd / band);
  return 0;
}
