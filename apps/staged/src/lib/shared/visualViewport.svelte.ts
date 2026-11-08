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
// Focus arriving in a text input re-measures at once. Focus leaving one waits
// for the visual viewport's next `resize` — the keyboard going away — or
// BLUR_SETTLE_MS, whichever comes first. iOS moves focus off the input on the
// synthetic `mousedown` of a tap and hit-tests the `click` again at the same
// point afterwards: re-measuring on the blur restyled the full-screen layout
// to the layout viewport in between, its bottom row (a Send or Start button)
// dropped by the keyboard's height, and the click landed on whatever slid
// under the finger instead. The keyboard starts to leave only once the tap is
// over, so by the time `resize` arrives the click has been delivered, and the
// layout follows the keyboard down instead of jumping ahead of it. The timer
// covers blurs with no on-screen keyboard to wait for (a hardware keyboard,
// a programmatic blur). While a blur is pending, viewport `scroll` events
// still undo the page scroll but leave the geometry alone.
//
// While anything is subscribed, the page scroll iOS Safari adds to reveal a
// focused input is undone on every viewport change. The subscriber's layout
// already keeps the input on screen, and a scrolled page would leave the
// rest of the shell (the TopBar, every hit target) offset under it. Where
// guardDocumentScroll keeps the scroll instead (shouldKeepDocumentScroll: a
// pinch-zoomed page, or a focused input outside the full-screen layout, such
// as a popover's search field over a dialog), so does this module, and the
// state stays at the layout viewport's geometry so the layout scrolls with
// the page and the input stays where Safari revealed it.
//
// Older webviews have no `visualViewport`; the state then stays at the
// layout viewport's geometry and behaviour is unchanged.

import { resetDocumentScroll, shouldKeepDocumentScroll } from './documentScroll';
import { isTextInput } from './textInput';
import {
  computeViewportGeometry,
  LAYOUT_VIEWPORT_GEOMETRY,
  type ViewportGeometry,
} from './viewportGeometry';

export const visibleViewport = $state<ViewportGeometry>({ ...LAYOUT_VIEWPORT_GEOMETRY });

/**
 * How long a blur waits for the keyboard's `resize` before re-measuring
 * anyway. It only has to outlast the tap's `click`, which follows the blur
 * within the same gesture; with an on-screen keyboard its `resize` normally
 * comes well before the timer.
 */
export const BLUR_SETTLE_MS = 500;

let trackedViewport: VisualViewport | null = null;
let subscriberCount = 0;
let initialSyncFrame = 0;
/** Fallback timer for a blur's re-measure; nonzero while one is pending. */
let blurSettleTimer: ReturnType<typeof setTimeout> | 0 = 0;

function measure(vv: VisualViewport, keepScroll: boolean): ViewportGeometry {
  return computeViewportGeometry({
    height: vv.height,
    offsetTop: vv.offsetTop,
    scale: vv.scale,
    layoutHeight: document.documentElement.clientHeight,
    // A kept scroll is what reveals the focused input; following the visual
    // viewport would carry the layout, and anything anchored in it, along
    // with the scroll and undo that.
    textInputFocused: !keepScroll && isTextInput(document.activeElement),
  });
}

function syncVisualViewport() {
  const vv = trackedViewport;
  if (!vv) return;
  const keepScroll = shouldKeepDocumentScroll();
  let geometry = measure(vv, keepScroll);
  if (!keepScroll) {
    resetDocumentScroll();
    // The visual viewport moves with the page; read where it landed.
    if (geometry.top !== 0) geometry = measure(vv, keepScroll);
  }
  // Unchanged fields are no-op writes on `$state`, so subscribers only
  // restyle when something actually moved.
  Object.assign(visibleViewport, geometry);
}

function cancelBlurSettle(): void {
  if (blurSettleTimer) clearTimeout(blurSettleTimer);
  blurSettleTimer = 0;
}

function settleBlur(): void {
  blurSettleTimer = 0;
  syncVisualViewport();
}

function handleViewportResize(): void {
  // The keyboard is moving (or the window resized): whatever a pending blur
  // was waiting for has happened.
  cancelBlurSettle();
  syncVisualViewport();
}

function handleViewportScroll(): void {
  if (blurSettleTimer) {
    // A blur is waiting for the keyboard to leave; the layout must not move
    // under a tap that may still be landing. The page scroll still goes.
    if (!shouldKeepDocumentScroll()) resetDocumentScroll();
    return;
  }
  syncVisualViewport();
}

function handleFocusChange(): void {
  if (isTextInput(document.activeElement)) {
    // Into a text input (including straight from another one, whose
    // `focusout` scheduled a settle): the keyboard is coming or staying.
    cancelBlurSettle();
    syncVisualViewport();
    return;
  }
  // Out of a text input: `focusout`, or `focusin` on whatever took focus
  // instead (a button, the dialog itself). Wait for the keyboard.
  if (!blurSettleTimer) blurSettleTimer = setTimeout(settleBlur, BLUR_SETTLE_MS);
}

export function watchVisualViewport(): () => void {
  if (typeof window === 'undefined' || !window.visualViewport) return () => {};

  subscriberCount += 1;

  if (!trackedViewport) {
    trackedViewport = window.visualViewport;
    trackedViewport.addEventListener('resize', handleViewportResize);
    trackedViewport.addEventListener('scroll', handleViewportScroll);
    document.addEventListener('focusin', handleFocusChange);
    document.addEventListener('focusout', handleFocusChange);
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
      cancelBlurSettle();
      trackedViewport?.removeEventListener('resize', handleViewportResize);
      trackedViewport?.removeEventListener('scroll', handleViewportScroll);
      document.removeEventListener('focusin', handleFocusChange);
      document.removeEventListener('focusout', handleFocusChange);
      trackedViewport = null;
      // Nothing keeps the state current any more; don't let the next
      // subscriber start from a stale keyboard.
      Object.assign(visibleViewport, LAYOUT_VIEWPORT_GEOMETRY);
    }
  };
}
