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
 *   braces, the next `click` on the trigger is swallowed on the capture phase
 *   for a short window, so nothing under the finger activates. The swallow is
 *   scoped to the trigger: the menu that just opened lives in a portal, so a
 *   quick tap on one of its rows is never eaten;
 * - a browser that raises its own trusted `contextmenu` for the hold a few
 *   milliseconds after ours already opened the menu (Android Chrome) has that
 *   duplicate eaten, so consumer handlers and bits-ui's opener run once;
 * - moving past the threshold, a second finger, or `touchcancel` (the page
 *   started scrolling) cancels a pending press and leaves the tap alone. A
 *   second finger after the press fired keeps the suppression, so the first
 *   finger's release still cannot activate anything.
 */

export const LONG_PRESS_DELAY_MS = 500;
export const LONG_PRESS_MOVE_THRESHOLD_PX = 10;
/**
 * How long after a long-press release a trailing click on the trigger is
 * still swallowed. The synthesized click arrives in the same task as
 * `touchend`; the rest is margin.
 */
export const CLICK_SWALLOW_WINDOW_MS = 100;

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
  /**
   * The trigger element: a trailing click is swallowed only when it is
   * dispatched to this element or something inside it, never to the menu.
   * Defaults to the element the press started on.
   */
  trigger?: () => EventTarget | null;
}

export interface LongPressController {
  ontouchstart(event: TouchEvent): void;
  ontouchmove(event: TouchEvent): void;
  ontouchend(event: TouchEvent): void;
  ontouchcancel(event: TouchEvent): void;
  /**
   * Returns `true` when the event was a duplicate of a press that already
   * opened the menu and has been eaten; the caller must then skip its own
   * consumer handler too.
   */
  oncontextmenu(event: MouseEvent): boolean;
  destroy(): void;
}

export function createLongPress(options: LongPressOptions): LongPressController {
  const delay = options.delay ?? LONG_PRESS_DELAY_MS;
  const moveThreshold = options.moveThreshold ?? LONG_PRESS_MOVE_THRESHOLD_PX;

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

  // Capture-phase on the trigger itself: it runs ahead of the handlers on
  // whatever descendant the click targets, and clicks elsewhere (the portalled
  // menu included) never pass through here.
  function armClickSwallow(node: EventTarget) {
    unarmClickSwallow?.();
    node.addEventListener('click', swallowClick, { capture: true });
    const expiry = setTimeout(() => unarmClickSwallow?.(), CLICK_SWALLOW_WINDOW_MS);
    unarmClickSwallow = () => {
      clearTimeout(expiry);
      node.removeEventListener('click', swallowClick, { capture: true });
      unarmClickSwallow = null;
    };
  }

  return {
    ontouchstart(event) {
      // A second finger is a pinch or scroll gesture, never a long press: drop
      // a pending press, but keep `fired` so the release of a press that
      // already opened the menu is still suppressed.
      if (event.touches.length !== 1) {
        clearTimer();
        return;
      }
      reset();
      if (options.isDisabled?.()) return;
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
      if (fired && start) {
        event.preventDefault();
        armClickSwallow(options.trigger?.() ?? start.target);
      }
      reset();
    },
    ontouchcancel() {
      reset();
    },
    oncontextmenu(event) {
      if (fired && event.isTrusted) {
        // The browser's own contextmenu for the hold, landing after ours
        // already opened the menu: a duplicate, not a new gesture.
        event.preventDefault();
        event.stopImmediatePropagation();
        return true;
      }
      // The browser (or our own synthetic event) opened the menu while the
      // finger is still down: treat the release the same as a fired press.
      if (start) {
        clearTimer();
        fired = true;
      }
      return false;
    },
    destroy() {
      reset();
      unarmClickSwallow?.();
    },
  };
}
