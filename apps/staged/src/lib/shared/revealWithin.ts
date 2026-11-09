/**
 * revealWithin — `Element.scrollIntoView`, stopped at a scroll box.
 *
 * scrollIntoView scrolls every scrolling ancestor of the element, up to the
 * page. Around the chat transcript those include the pane itself, which
 * scrolls when a short screen leaves it too little room, and the dialog, so
 * revealing a row in the transcript could push the composer under the
 * on-screen keyboard. This scrolls the scroll containers between the element
 * and `box` (a code block a search match sits in, say), then `box` itself,
 * aligning the element the way scrollIntoView would. Nothing outside `box`
 * scrolls.
 *
 * Assumes the boxes scroll forward (no `column-reverse` or right-to-left
 * scrolling), as the transcript and the blocks in it do.
 */

export type RevealAlign = 'start' | 'center' | 'end' | 'nearest';

export interface RevealOptions {
  block?: RevealAlign;
  inline?: RevealAlign;
  behavior?: ScrollBehavior;
}

/**
 * How far a scroll container must scroll along one axis to align the span
 * [start, end] in its scrollport [portStart, portEnd]. `nearest` follows
 * scrollIntoView: nothing when the span is in view or covers the port;
 * otherwise the edge it overflows past is aligned, or the opposite edge when
 * the span is larger than the port.
 */
export function alignDelta(
  align: RevealAlign,
  start: number,
  end: number,
  portStart: number,
  portEnd: number
): number {
  switch (align) {
    case 'start':
      return start - portStart;
    case 'end':
      return end - portEnd;
    case 'center':
      return (start + end - portStart - portEnd) / 2;
    case 'nearest': {
      const size = end - start;
      const port = portEnd - portStart;
      const before = start < portStart;
      const after = end > portEnd;
      if (before && after) return 0;
      if ((before && size < port) || (after && size > port)) return start - portStart;
      if ((before && size > port) || (after && size < port)) return end - portEnd;
      return 0;
    }
  }
}

/** Any box whose overflow isn't visible or clip, as scrollIntoView counts them. */
function isScrollContainer(node: Element): boolean {
  const { overflowX, overflowY } = getComputedStyle(node);
  const scrolls = (overflow: string) => overflow !== 'visible' && overflow !== 'clip';
  return scrolls(overflowX) || scrolls(overflowY);
}

/** The part of `delta` a box can take from `position`, within [0, max]. */
function reachable(position: number, delta: number, max: number): number {
  return Math.min(Math.max(position + delta, 0), Math.max(max, 0)) - position;
}

/**
 * Scrolls `el` into view inside `box` only. An element that isn't inside
 * `box` is left alone. `isScroller` is injectable for tests.
 */
export function revealWithin(
  el: Element,
  box: Element,
  { block = 'start', inline = 'nearest', behavior = 'auto' }: RevealOptions = {},
  isScroller: (node: Element) => boolean = isScrollContainer
): void {
  if (el === box || !box.contains(el)) return;
  let { top, bottom, left, right } = el.getBoundingClientRect();
  for (let node = el.parentElement; node; node = node.parentElement) {
    if (node === box || isScroller(node)) {
      const frame = node.getBoundingClientRect();
      const portTop = frame.top + node.clientTop;
      const portLeft = frame.left + node.clientLeft;
      const dy = reachable(
        node.scrollTop,
        alignDelta(block, top, bottom, portTop, portTop + node.clientHeight),
        node.scrollHeight - node.clientHeight
      );
      const dx = reachable(
        node.scrollLeft,
        alignDelta(inline, left, right, portLeft, portLeft + node.clientWidth),
        node.scrollWidth - node.clientWidth
      );
      if (dx !== 0 || dy !== 0) {
        node.scrollTo({ top: node.scrollTop + dy, left: node.scrollLeft + dx, behavior });
      }
      // Where the element will be once this box has scrolled (a smooth
      // scroll hasn't moved it yet), for the boxes around this one.
      top -= dy;
      bottom -= dy;
      left -= dx;
      right -= dx;
    }
    if (node === box) return;
  }
}
