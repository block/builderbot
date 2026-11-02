// visualViewport.svelte.ts — track the on-screen part of the page.
//
// Exposes computeViewportGeometry()'s reading of `window.visualViewport` as
// reactive state (`visibleViewport`), so a full-screen layout can cover
// exactly what is on screen while the on-screen keyboard is up and pin its
// footer to the keyboard's top edge. Mirrors the watch/unsubscribe pattern of
// viewport.svelte.ts.
//
// Subscribers apply the geometry as inline style on their own element rather
// than reading custom properties off `:root`: any write to the root's `style`
// attribute restyles the whole document and fires the MutationObserver in
// isDark.svelte.ts, and the viewport moves on every frame of the keyboard's
// slide.
//
// The state follows focus too (see viewportGeometry.ts): it is the visual
// viewport only while a text input has focus, so focus changes re-measure.
//
// While anything is subscribed, the page scroll iOS Safari adds to reveal a
// focused input is undone on every viewport change. The subscriber's layout
// already keeps the input on screen, and a scrolled page would leave the
// rest of the shell (the TopBar, every hit target) offset under it.
//
// Older webviews have no `visualViewport`; the state then stays at the
// layout viewport's geometry and behaviour is unchanged.

import { resetDocumentScroll } from './documentScroll';
import {
  computeViewportGeometry,
  LAYOUT_VIEWPORT_GEOMETRY,
  type ViewportGeometry,
} from './viewportGeometry';

export const visibleViewport = $state<ViewportGeometry>({ ...LAYOUT_VIEWPORT_GEOMETRY });

let trackedViewport: VisualViewport | null = null;
let subscriberCount = 0;
let initialSyncFrame = 0;

// Input types that open no keyboard (buttons, toggles, pickers iOS draws in
// its own popover).
const NON_TEXT_INPUT_TYPES = new Set([
  'button',
  'checkbox',
  'color',
  'file',
  'hidden',
  'image',
  'radio',
  'range',
  'reset',
  'submit',
]);

function isTextInput(el: Element | null): boolean {
  if (!el) return false;
  if (el.tagName === 'TEXTAREA') return true;
  if (el.tagName === 'INPUT') return !NON_TEXT_INPUT_TYPES.has((el as HTMLInputElement).type);
  return (el as HTMLElement).isContentEditable === true;
}

function measure(vv: VisualViewport): ViewportGeometry {
  return computeViewportGeometry({
    height: vv.height,
    offsetTop: vv.offsetTop,
    scale: vv.scale,
    layoutHeight: document.documentElement.clientHeight,
    textInputFocused: isTextInput(document.activeElement),
  });
}

function syncVisualViewport() {
  const vv = trackedViewport;
  if (!vv) return;
  let geometry = measure(vv);
  // A pinch-zoomed viewport is panned over the page on purpose; leave it.
  if (!geometry.zoomed) {
    resetDocumentScroll();
    // The visual viewport moves with the page; read where it landed.
    if (geometry.top !== 0) geometry = measure(vv);
  }
  // Unchanged fields are no-op writes on `$state`, so subscribers only
  // restyle when something actually moved.
  Object.assign(visibleViewport, geometry);
}

export function watchVisualViewport(): () => void {
  if (typeof window === 'undefined' || !window.visualViewport) return () => {};

  subscriberCount += 1;

  if (!trackedViewport) {
    trackedViewport = window.visualViewport;
    trackedViewport.addEventListener('resize', syncVisualViewport);
    trackedViewport.addEventListener('scroll', syncVisualViewport);
    document.addEventListener('focusin', syncVisualViewport);
    document.addEventListener('focusout', syncVisualViewport);
  }
  // The initial read forces a style/layout flush. Subscribers call this as
  // they mount — e.g. a dialog that has just been portalled in — so reading
  // synchronously would make the mount flush pay that recalc before the
  // first frame can paint. Defer it to the next frame instead; until then
  // the state holds the layout geometry, which is right without a keyboard.
  if (!initialSyncFrame) {
    initialSyncFrame = requestAnimationFrame(() => {
      initialSyncFrame = 0;
      syncVisualViewport();
    });
  }

  return () => {
    subscriberCount = Math.max(0, subscriberCount - 1);
    if (subscriberCount === 0) {
      cancelAnimationFrame(initialSyncFrame);
      initialSyncFrame = 0;
      trackedViewport?.removeEventListener('resize', syncVisualViewport);
      trackedViewport?.removeEventListener('scroll', syncVisualViewport);
      document.removeEventListener('focusin', syncVisualViewport);
      document.removeEventListener('focusout', syncVisualViewport);
      trackedViewport = null;
      // Nothing keeps the state current any more; don't let the next
      // subscriber start from a stale keyboard.
      Object.assign(visibleViewport, LAYOUT_VIEWPORT_GEOMETRY);
    }
  };
}
