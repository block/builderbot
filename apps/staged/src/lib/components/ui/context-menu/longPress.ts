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
 * - moving past the threshold, a second finger, or `touchcancel` cancels a
 *   pending press and leaves the tap alone. A second finger after the press
 *   fired keeps the suppression, so the first finger's release still cannot
 *   activate anything;
 * - a scroll cancels a pending press too. iOS Safari sends no `touchcancel`
 *   when it turns a touch into a scroll: it sends `pointercancel`, and the
 *   touch events carry on. A slow drag can stay inside `moveThreshold` until
 *   after the scroll has begun, so the pending press is also dropped on
 *   `pointercancel` and on any `scroll` of a container holding the touched
 *   element. A press that already fired keeps its suppression;
 * - a touch that lands on a list that is still moving is the user catching
 *   the scroll, not pressing a row: a `touchstart` within
 *   `SCROLL_CATCH_WINDOW_MS` of the last scroll of a container holding the
 *   touched element starts no press at all.
 */

export const LONG_PRESS_DELAY_MS = 500;
export const LONG_PRESS_MOVE_THRESHOLD_PX = 10;
/**
 * How long after a long-press release a trailing click is still swallowed.
 * The synthesized click arrives in the same task as `touchend`; the rest is
 * margin.
 */
export const CLICK_SWALLOW_WINDOW_MS = 100;
/**
 * How recently a container must have scrolled for a touch landing in it to
 * count as catching the scroll. Momentum scrolling reports a `scroll` every
 * frame, so anything still moving is well inside this.
 */
export const SCROLL_CATCH_WINDOW_MS = 150;

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

/**
 * Whether scrolling `scroller` moves `target` on screen: the document does
 * for everything, an element for what it contains. Scrolls of an unrelated
 * pane (a transcript following its stream, say) leave the press alone.
 */
function scrollMoves(scroller: EventTarget | null, target: EventTarget): boolean {
  if (!scroller || scroller === target) return true;
  const contains = (scroller as Partial<Node>).contains;
  if (typeof contains !== 'function') return true;
  return contains.call(scroller, target as Node);
}

/** Recent scrolls seen on one document, shared by every controller on it. */
interface ScrollWatch {
  users: number;
  /** When each container last scrolled; emptied once a burst of scrolling ends. */
  recent: Map<EventTarget | null, number>;
  latest: number;
  stop(): void;
}

const scrollWatches = new WeakMap<EventTarget, ScrollWatch>();

// One passive capture listener per document, however many triggers are
// mounted. `scroll` does not bubble, so capture is what sees a nested
// scroller from the document.
function watchScrolls(scope: EventTarget): ScrollWatch {
  let watch = scrollWatches.get(scope);
  if (!watch) {
    const recent: ScrollWatch['recent'] = new Map();
    const opts = { capture: true, passive: true };
    const onScroll = (event: Event) => {
      const now = Date.now();
      if (now - created.latest > SCROLL_CATCH_WINDOW_MS) recent.clear();
      recent.set(event.target, now);
      created.latest = now;
    };
    const created: ScrollWatch = {
      users: 0,
      recent,
      latest: 0,
      stop() {
        scope.removeEventListener('scroll', onScroll, opts);
        scrollWatches.delete(scope);
        recent.clear();
      },
    };
    scope.addEventListener('scroll', onScroll, opts);
    scrollWatches.set(scope, created);
    watch = created;
  }
  watch.users += 1;
  return watch;
}

function unwatchScrolls(watch: ScrollWatch) {
  watch.users -= 1;
  if (watch.users === 0) watch.stop();
}

/** Whether a container holding `target` scrolled within the catch window. */
function landedOnMovingList(watch: ScrollWatch, target: EventTarget): boolean {
  const now = Date.now();
  if (now - watch.latest > SCROLL_CATCH_WINDOW_MS) return false;
  for (const [scroller, at] of watch.recent) {
    if (now - at <= SCROLL_CATCH_WINDOW_MS && scrollMoves(scroller, target)) return true;
  }
  return false;
}

export function createLongPress(options: LongPressOptions): LongPressController {
  const delay = options.delay ?? LONG_PRESS_DELAY_MS;
  const moveThreshold = options.moveThreshold ?? LONG_PRESS_MOVE_THRESHOLD_PX;

  let start: LongPressPoint | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let fired = false;
  let unlistenGesture: (() => void) | null = null;
  let unarmClickSwallow: (() => void) | null = null;

  const watchScope = options.scope?.() ?? (typeof document === 'undefined' ? null : document);
  let scrollWatch = watchScope ? watchScrolls(watchScope) : null;

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

  // The browser took the touch for itself, which on iOS means a scroll began.
  // Unlike `touchcancel` the touch events keep coming, so only the pending
  // press goes: the release of one that already fired is still suppressed.
  function onpointercancel(event: Event) {
    const type = (event as PointerEvent).pointerType;
    if (type === undefined || type === 'touch') clearTimer();
  }

  function onscroll(event: Event) {
    if (start && timer !== null && scrollMoves(event.target, start.target)) clearTimer();
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
    const passive = { capture: true, passive: true };
    const nodes = target === scope ? [scope] : [scope, target];
    for (const node of nodes) {
      node.addEventListener('touchmove', ontouchmove as EventListener, opts);
      node.addEventListener('touchend', ontouchend as EventListener, opts);
      node.addEventListener('touchcancel', ontouchcancel, opts);
      node.addEventListener('pointercancel', onpointercancel, passive);
    }
    scope.addEventListener('scroll', onscroll, passive);
    scope.addEventListener('touchstart', ontouchstartAnywhere as EventListener, opts);
    unlistenGesture = () => {
      for (const node of nodes) {
        node.removeEventListener('touchmove', ontouchmove as EventListener, opts);
        node.removeEventListener('touchend', ontouchend as EventListener, opts);
        node.removeEventListener('touchcancel', ontouchcancel, opts);
        node.removeEventListener('pointercancel', onpointercancel, passive);
      }
      scope.removeEventListener('scroll', onscroll, passive);
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
      const target = event.target ?? touch.target;
      // Catching a list that is still moving stops it; it presses nothing.
      if (scrollWatch && landedOnMovingList(scrollWatch, target)) return;
      start = { target, clientX: touch.clientX, clientY: touch.clientY };
      const point = start;
      const scope = scopeFor(point.target);
      if (scope) listenGesture(scope, point.target);
      timer = setTimeout(() => {
        timer = null;
        fired = true;
        const fireAt = isDetached(point.target)
          ? (options.trigger?.() ?? point.target)
          : point.target;
        options.onLongPress({ ...point, target: fireAt });
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
      if (scrollWatch) unwatchScrolls(scrollWatch);
      scrollWatch = null;
    },
  };
}
