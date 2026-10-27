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
 *   `oncontextmenu` handlers). The event is dispatched at the touched element,
 *   or at the trigger itself when that element left the DOM during the hold;
 * - once a long press fired — ours, or the browser's own `contextmenu` while
 *   the touch was down — the closing `touchend` is default-prevented. That is
 *   the mechanism that suppresses the synthesized click on iOS and Chrome, in
 *   every presentation: with a bottom sheet open the click would hit-test the
 *   sheet layer, with a floating menu the trigger underneath. Belt and braces,
 *   the next `click` anywhere in the document is swallowed on the capture
 *   phase for a short window — unless a new touch has started since, which
 *   marks it as the user's next tap rather than the release's leftover;
 * - the rest of the gesture (`touchmove`, `touchend`, `touchcancel`) is
 *   listened for on the document *and* on the touched element itself while a
 *   press is live. Touch events are retargeted to the original `touchstart`
 *   target and their path is built from that node's ancestors at dispatch
 *   time, so a descendant replaced mid-hold (a keyed re-render) delivers the
 *   release to nothing but itself — neither the trigger nor the document ever
 *   sees it. Whichever listener fires first handles the release and tears all
 *   of them down. Should a release still go unseen, the next single-finger
 *   `touchstart` anywhere in the document while a press is on record means
 *   the earlier finger already lifted: the stale press is dropped before the
 *   new tap is looked at, so it cannot eat that tap's release;
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
 * How long after a long-press release a trailing click is still swallowed.
 * The synthesized click arrives in the same task as `touchend`; the rest is
 * margin.
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
   * The trigger element. A press whose touched descendant is no longer in the
   * DOM when the hold matures fires at the trigger instead, so the menu still
   * opens for the element the user is holding.
   */
  trigger?: () => EventTarget | null;
  /**
   * Where the gesture-wide listeners live: the document. Defaults to the
   * pressed element's owner document; tests supply a stand-in.
   */
  scope?: () => EventTarget | null;
}

export interface LongPressController {
  ontouchstart(event: TouchEvent): void;
  /**
   * Returns `true` when the event was a duplicate of a press that already
   * opened the menu and has been eaten; the caller must then skip its own
   * consumer handler too.
   */
  oncontextmenu(event: MouseEvent): boolean;
  destroy(): void;
}

function isDetached(target: EventTarget): boolean {
  return 'isConnected' in target && !(target as Node).isConnected;
}

export function createLongPress(options: LongPressOptions): LongPressController {
  const delay = options.delay ?? LONG_PRESS_DELAY_MS;
  const moveThreshold = options.moveThreshold ?? LONG_PRESS_MOVE_THRESHOLD_PX;

  let start: LongPressPoint | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let fired = false;
  let unlistenGesture: (() => void) | null = null;
  let unarmClickSwallow: (() => void) | null = null;

  function scopeFor(target: EventTarget): EventTarget | null {
    return (
      options.scope?.() ??
      (target as Node).ownerDocument ??
      (typeof document === 'undefined' ? null : document)
    );
  }

  function clearTimer() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  // Drops every reference to the gesture, the touched node included: a press
  // whose target left the DOM must not keep it alive past its release.
  function reset() {
    clearTimer();
    unlistenGesture?.();
    start = null;
    fired = false;
  }

  function swallowClick(event: Event) {
    event.preventDefault();
    event.stopImmediatePropagation();
    unarmClickSwallow?.();
  }

  // Capture phase on the document: it runs ahead of the handlers on whatever
  // the synthesized click targets — the trigger's row, or the sheet layer that
  // now covers it. A new touchstart means the next click belongs to a fresh
  // tap (on a row of the menu that just opened, say), so it disarms the
  // swallow instead of being eaten by it.
  function armClickSwallow(scope: EventTarget) {
    unarmClickSwallow?.();
    const disarm = () => unarmClickSwallow?.();
    scope.addEventListener('click', swallowClick, { capture: true });
    scope.addEventListener('touchstart', disarm, { capture: true });
    const expiry = setTimeout(disarm, CLICK_SWALLOW_WINDOW_MS);
    unarmClickSwallow = () => {
      clearTimeout(expiry);
      scope.removeEventListener('click', swallowClick, { capture: true });
      scope.removeEventListener('touchstart', disarm, { capture: true });
      unarmClickSwallow = null;
    };
  }

  // A touchstart reaching the document while a press is on record. One
  // finger: the pressing finger must already have lifted without us seeing
  // the release (its target was detached), so the stale press is dropped
  // before this tap's own touchend — which would otherwise be eaten — is
  // looked at. Two or more: a second finger, which drops a pending press
  // wherever it landed, not only on the trigger.
  function ontouchstartAnywhere(event: TouchEvent) {
    if (!start) return;
    if (event.touches.length === 1) {
      reset();
      return;
    }
    clearTimer();
  }

  function ontouchmove(event: TouchEvent) {
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
  }

  function ontouchend(event: TouchEvent) {
    if (!start) return;
    if (fired) {
      event.preventDefault();
      const scope = scopeFor(start.target);
      if (scope) armClickSwallow(scope);
    }
    reset();
  }

  function ontouchcancel() {
    reset();
  }

  // The listeners live only for the duration of a press, on the document and
  // on the touched node (the only place a release for a since-detached node
  // is delivered). For a node still in the DOM both see the same event, the
  // document's capture listener first; it resets the press, which removes
  // the node's listener before the event reaches it. `touchend` must not be
  // passive so the release can be default-prevented.
  function listenGesture(scope: EventTarget, target: EventTarget) {
    unlistenGesture?.();
    const opts = { capture: true, passive: false };
    const nodes = target === scope ? [scope] : [scope, target];
    for (const node of nodes) {
      node.addEventListener('touchmove', ontouchmove as EventListener, opts);
      node.addEventListener('touchend', ontouchend as EventListener, opts);
      node.addEventListener('touchcancel', ontouchcancel, opts);
    }
    scope.addEventListener('touchstart', ontouchstartAnywhere as EventListener, opts);
    unlistenGesture = () => {
      for (const node of nodes) {
        node.removeEventListener('touchmove', ontouchmove as EventListener, opts);
        node.removeEventListener('touchend', ontouchend as EventListener, opts);
        node.removeEventListener('touchcancel', ontouchcancel, opts);
      }
      scope.removeEventListener('touchstart', ontouchstartAnywhere as EventListener, opts);
      unlistenGesture = null;
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
      const scope = scopeFor(point.target);
      if (scope) listenGesture(scope, point.target);
      timer = setTimeout(() => {
        timer = null;
        fired = true;
        const target = isDetached(point.target)
          ? (options.trigger?.() ?? point.target)
          : point.target;
        options.onLongPress({ ...point, target });
      }, delay);
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
