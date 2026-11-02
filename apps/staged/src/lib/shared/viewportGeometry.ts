// viewportGeometry.ts — where the on-screen part of the page is, for layouts
// that have to stay inside it while the on-screen keyboard is up.
//
// iOS WebKit does not shrink the layout viewport (or `100dvh`) for the
// keyboard: it shrinks the visual viewport, and to reveal a focused input it
// scrolls the page even though html/body are `overflow: hidden`. A
// `position: fixed` box is laid out against the layout viewport, so to cover
// exactly what is on screen it takes the visual viewport's `offsetTop` as its
// top and the visual viewport's height as its height.
//
// The visual viewport is only followed while a text input has focus, the
// one time the page can have a keyboard over it. Otherwise the layout
// viewport is the screen. iOS can also leave the visual viewport stale: a
// keyboard raised by a system sheet (Add to Home Screen) and still up when
// Safari went to the background stays subtracted from it after Safari
// returns, with nothing covering the page.

/**
 * Smallest bottom inset read as an on-screen keyboard. Anything less is
 * browser chrome changing size (the input accessory bar shown over a hardware
 * keyboard sits where Safari's toolbar was), not something covering the page.
 */
const KEYBOARD_MIN_INSET_PX = 100;

/**
 * Above this scale the user has pinch-zoomed (iOS ignores `user-scalable=no`):
 * the visual viewport is then a magnified window onto the page, not the
 * screen, and a layout that followed it would shrink under the zoom.
 */
const MAX_UNZOOMED_SCALE = 1.01;

export interface ViewportSample {
  /** `visualViewport.height` */
  height: number;
  /** `visualViewport.offsetTop` */
  offsetTop: number;
  /** `visualViewport.scale` */
  scale: number;
  /**
   * The layout viewport's height, `document.documentElement.clientHeight`.
   * iOS leaves it alone when the keyboard opens.
   */
  layoutHeight: number;
  /** The focused element takes typing, so an on-screen keyboard may be up. */
  textInputFocused: boolean;
}

export interface ViewportGeometry {
  /** `top` for a `position: fixed` box that starts at the top of the screen. */
  top: number;
  /** Height of the on-screen part of the page; null means use `100dvh`. */
  height: number | null;
  /** An on-screen keyboard covers the bottom of the layout viewport. */
  keyboardOpen: boolean;
  /** Pinch-zoomed: the geometry falls back to the layout viewport. */
  zoomed: boolean;
}

/** The geometry before anything is measured, or with no `visualViewport`. */
export const LAYOUT_VIEWPORT_GEOMETRY: Readonly<ViewportGeometry> = {
  top: 0,
  height: null,
  keyboardOpen: false,
  zoomed: false,
};

/** The user has pinch-zoomed the page (`visualViewport.scale`). */
export function isPinchZoomed(scale: number): boolean {
  return scale > MAX_UNZOOMED_SCALE;
}

export function computeViewportGeometry(sample: ViewportSample): ViewportGeometry {
  if (isPinchZoomed(sample.scale)) {
    return { ...LAYOUT_VIEWPORT_GEOMETRY, zoomed: true };
  }
  if (!sample.textInputFocused) return { ...LAYOUT_VIEWPORT_GEOMETRY };
  return {
    top: Math.max(0, sample.offsetTop),
    height: sample.height,
    keyboardOpen: sample.layoutHeight - sample.height >= KEYBOARD_MIN_INSET_PX,
    zoomed: false,
  };
}
