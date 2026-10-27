/**
 * Touch side of a context-menu long press.
 *
 * bits-ui opens a context menu after a 700 ms touch hold but leaves the rest
 * of the gesture alone: lifting the finger still fires `touchend` and the
 * browser's synthesized `click` on whatever is under it — the trigger's own
 * button or row, or the menu that just appeared — so the item activates (or a
 * menu entry fires) and the menu is unusable on mobile web. This controller
 * owns the touch gesture instead:
 *
 * - a single touch that stays within `moveThreshold` for `delay` ms fires
 *   `onLongPress`; the trigger turns that into a synthetic `contextmenu` so
 *   bits-ui opens through its normal path (positioning, focus, consumer
 *   `oncontextmenu` handlers);
 * - once a long press fired — ours, or the browser's own `contextmenu` while
 *   the touch was down — the closing `touchend` is default-prevented (that is
 *   what suppresses the synthesized click on iOS and Chrome) and, belt and
 *   braces, the next `click` is swallowed on the capture phase for a short
 *   window, so nothing under the finger activates and the just-opened menu is
 *   not dismissed by the same gesture;
 * - moving past the threshold, a second finger, or `touchcancel` (the page
 *   started scrolling) cancels the press and leaves the tap alone.
 */

export const LONG_PRESS_DELAY_MS = 500;
export const LONG_PRESS_MOVE_THRESHOLD_PX = 10;
/** How long after a long-press release a stray click is still swallowed. */
export const CLICK_SWALLOW_WINDOW_MS = 500;

export interface LongPressPoint {
  target: EventTarget;
  clientX: number;
  clientY: number;
}

export interface LongPressOptions {
  onLongPress: (point: LongPressPoint) => void;
  isDisabled?: () => boolean;
  delay?: number;
  moveThreshold?: number;
  /** Where the trailing click is intercepted; defaults to `window`. */
  clickTarget?: EventTarget | null;
}

export interface LongPressController {
  ontouchstart(event: TouchEvent): void;
  ontouchmove(event: TouchEvent): void;
  ontouchend(event: TouchEvent): void;
  ontouchcancel(event: TouchEvent): void;
  oncontextmenu(event: MouseEvent): void;
  destroy(): void;
}

export function createLongPress(options: LongPressOptions): LongPressController {
  const delay = options.delay ?? LONG_PRESS_DELAY_MS;
  const moveThreshold = options.moveThreshold ?? LONG_PRESS_MOVE_THRESHOLD_PX;
  const clickTarget =
    options.clickTarget !== undefined
      ? options.clickTarget
      : typeof window !== 'undefined'
        ? window
        : null;

  let start: LongPressPoint | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let fired = false;
  let unarmClickSwallow: (() => void) | null = null;

  function clearTimer() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  function reset() {
    clearTimer();
    start = null;
    fired = false;
  }

  function swallowClick(event: Event) {
    event.preventDefault();
    event.stopImmediatePropagation();
    unarmClickSwallow?.();
  }

  function armClickSwallow() {
    unarmClickSwallow?.();
    if (!clickTarget) return;
    clickTarget.addEventListener('click', swallowClick, { capture: true });
    const expiry = setTimeout(() => unarmClickSwallow?.(), CLICK_SWALLOW_WINDOW_MS);
    unarmClickSwallow = () => {
      clearTimeout(expiry);
      clickTarget.removeEventListener('click', swallowClick, { capture: true });
      unarmClickSwallow = null;
    };
  }

  return {
    ontouchstart(event) {
      reset();
      if (options.isDisabled?.()) return;
      // A second finger is a pinch or scroll gesture, never a long press.
      if (event.touches.length !== 1) return;
      const touch = event.touches[0];
      start = {
        target: event.target ?? touch.target,
        clientX: touch.clientX,
        clientY: touch.clientY,
      };
      const point = start;
      timer = setTimeout(() => {
        timer = null;
        fired = true;
        options.onLongPress(point);
      }, delay);
    },
    ontouchmove(event) {
      if (!start) return;
      if (event.touches.length !== 1) {
        clearTimer();
        return;
      }
      const touch = event.touches[0];
      const dx = touch.clientX - start.clientX;
      const dy = touch.clientY - start.clientY;
      // Finger drift under the threshold is jitter, not intent. Past it the
      // pending press is dropped; a press that already fired keeps its
      // suppression so the release still cannot activate anything.
      if (dx * dx + dy * dy > moveThreshold * moveThreshold) clearTimer();
    },
    ontouchend(event) {
      if (fired) {
        event.preventDefault();
        armClickSwallow();
      }
      reset();
    },
    ontouchcancel() {
      reset();
    },
    oncontextmenu() {
      // The browser (or our own synthetic event) opened the menu while the
      // finger is still down: treat the release the same as a fired press.
      if (!start) return;
      clearTimer();
      fired = true;
    },
    destroy() {
      reset();
      unarmClickSwallow?.();
    },
  };
}
