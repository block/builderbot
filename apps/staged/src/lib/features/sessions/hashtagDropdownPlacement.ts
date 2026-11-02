import type { ViewportRect } from '../../shared/visibleViewportRect';

/** Gap between the caret line and the dropdown. */
export const CARET_GAP_PX = 4;
/** Room kept between the dropdown and the top or bottom of the visible viewport. */
export const VIEWPORT_MARGIN_PX = 8;
/** Room kept on either side, as `.hashtag-dropdown { max-width: calc(100vw - 32px) }`. */
export const SIDE_MARGIN_PX = 16;
/** Never shrink the list below this, even if it then runs off screen. */
export const MIN_DROPDOWN_HEIGHT_PX = 96;

export interface DropdownPlacement {
  side: 'above' | 'below';
  /**
   * Fixed `top` in client coordinates. Below the caret it is the dropdown's
   * top edge; above, its bottom edge (pair with `translate: 0 -100%`), so the
   * dropdown never has to be positioned from the bottom of a layout viewport
   * that iOS does not shrink for the keyboard.
   */
  top: number;
  left: number;
  width: number;
  maxHeight: number;
}

/**
 * Open the hashtag dropdown on whichever side of the caret has room for it in
 * the visible viewport (`visibleViewportRect()`), preferring below, and cap
 * its height to the space on that side. It starts at the caret unless that
 * would run it past the right edge of the screen.
 */
export function placeHashtagDropdown(
  caret: { top: number; bottom: number; left: number },
  visible: ViewportRect,
  size: { width: number; maxHeight: number }
): DropdownPlacement {
  const width = Math.min(size.width, visible.right - visible.left - SIDE_MARGIN_PX * 2);
  const left = Math.max(visible.left, Math.min(caret.left, visible.right - SIDE_MARGIN_PX - width));

  const spaceBelow = visible.bottom - caret.bottom;
  const spaceAbove = caret.top - visible.top;
  const capped = (space: number) =>
    Math.min(
      size.maxHeight,
      Math.max(MIN_DROPDOWN_HEIGHT_PX, space - CARET_GAP_PX - VIEWPORT_MARGIN_PX)
    );
  if (spaceBelow >= size.maxHeight || spaceBelow > spaceAbove) {
    return {
      side: 'below',
      top: caret.bottom + CARET_GAP_PX,
      left,
      width,
      maxHeight: capped(spaceBelow),
    };
  }
  return {
    side: 'above',
    top: caret.top - CARET_GAP_PX,
    left,
    width,
    maxHeight: capped(spaceAbove),
  };
}
