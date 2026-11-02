export interface ViewportRect {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/**
 * The part of the page that is on screen, in client coordinates (the same
 * ones `getBoundingClientRect()` and `position: fixed` use).
 *
 * On iOS that is the visual viewport. With the keyboard up, `innerHeight`
 * shrinks to its height but not its position: Safari may have scrolled the
 * page so that it starts `offsetTop` into the layout viewport. Pinch zoom
 * shrinks and pans it the same way. Fixed positioning keeps measuring from
 * the layout viewport, which the keyboard does not shrink. Without
 * `visualViewport`, and wherever the two viewports agree (desktop), this is
 * the window.
 */
export function visibleViewportRect(): ViewportRect {
  const vv = window.visualViewport;
  if (!vv) return { top: 0, right: window.innerWidth, bottom: window.innerHeight, left: 0 };
  return {
    top: vv.offsetTop,
    right: vv.offsetLeft + vv.width,
    bottom: vv.offsetTop + vv.height,
    left: vv.offsetLeft,
  };
}
