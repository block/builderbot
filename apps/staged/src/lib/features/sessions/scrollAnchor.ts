/**
 * Scroll anchoring for the transcript box.
 *
 * A pixel `scrollTop` only names a place in the transcript while the box and
 * its rows keep their geometry. Across a park (the NoteModal aside and the
 * SessionModal are different widths, and either can be resized) or a trim
 * that removes rows above the viewport, the stable description of "where the
 * user is" is the first group in view plus its offset from the top of the
 * box: restore puts that group back at that offset whatever reflowed.
 *
 * Groups are rendered with `data-group-key` (see `transcriptGroupKey`) so an
 * anchor survives the keyed each re-creating the element.
 *
 * Positions are read from layout offsets (`offsetTop`), not client rects: the
 * restore runs while the dialog's entry animation still has the content
 * scaled, and a rect difference taken then is scaled with it (a 0.95 zoom
 * put a mid-transcript restore 38px short). Layout offsets ignore transforms.
 */

export interface ScrollAnchor {
  /** `data-group-key` of the first group whose bottom edge is in the box. */
  key: string;
  /** That group's top edge relative to the box's top edge, px. */
  offset: number;
}

/**
 * Index of the first of `count` groups, in document order, whose bottom edge
 * sits below `boxTop`; -1 when none does. Bottoms are monotonic in document
 * order, so this is a binary search — the scroll handler calls it per event.
 */
export function firstVisibleIndex(
  count: number,
  bottomAt: (index: number) => number,
  boxTop: number
): number {
  let lo = 0;
  let hi = count;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (bottomAt(mid) > boxTop) hi = mid;
    else lo = mid + 1;
  }
  return lo < count ? lo : -1;
}

/**
 * The `scrollTop` that puts the anchored group back at `anchor.offset`, given
 * `anchorTop`, its top edge relative to the box's top edge now: scrolling
 * down by `d` moves every top edge up by `d`. Callers clamp; the box does the
 * same on assignment.
 */
export function anchoredScrollTop(
  scrollTop: number,
  anchor: ScrollAnchor,
  anchorTop: number
): number {
  return scrollTop + anchorTop - anchor.offset;
}

const GROUP_SELECTOR = '.transcript-group';

/** Layout position of `el`'s border-box top in the coordinate space its
 *  outermost offset parent uses — the same space for every element under
 *  one dialog, so differences are meaningful; scroll offsets and transforms
 *  do not enter. */
function layoutTop(el: HTMLElement): number {
  let y = 0;
  for (let node: HTMLElement | null = el; node; node = node.offsetParent as HTMLElement | null) {
    y += node.offsetTop;
  }
  return y;
}

/** Top edge of `el` relative to the visible top edge of `box`, px. */
function topWithin(el: HTMLElement, box: HTMLElement, boxTop = layoutTop(box)): number {
  return layoutTop(el) - boxTop - box.scrollTop;
}

/** The anchor for `box` as laid out now; null while it shows no group. */
export function captureScrollAnchor(box: HTMLElement): ScrollAnchor | null {
  const groups = box.querySelectorAll<HTMLElement>(GROUP_SELECTOR);
  if (groups.length === 0) return null;
  const boxTop = layoutTop(box);
  const index = firstVisibleIndex(
    groups.length,
    (i) => topWithin(groups[i], box, boxTop) + groups[i].offsetHeight,
    0
  );
  if (index < 0) return null;
  const group = groups[index];
  const key = group.dataset.groupKey;
  if (key === undefined) return null;
  return { key, offset: topWithin(group, box, boxTop) };
}

/** Top edge of the group `anchor` names relative to the box's visible top,
 *  or null once it is gone. */
export function anchorTopIn(box: HTMLElement, anchor: ScrollAnchor): number | null {
  for (const group of box.querySelectorAll<HTMLElement>(GROUP_SELECTOR)) {
    if (group.dataset.groupKey === anchor.key) return topWithin(group, box);
  }
  return null;
}

/**
 * Scroll `box` so the group `anchor` names is back at its offset. Returns
 * false when that group is no longer rendered, so the caller can pick a
 * fallback (the bottom, a clamped offset).
 */
export function restoreScrollAnchor(box: HTMLElement, anchor: ScrollAnchor): boolean {
  const top = anchorTopIn(box, anchor);
  if (top === null) return false;
  box.scrollTop = anchoredScrollTop(box.scrollTop, anchor, top);
  return true;
}
