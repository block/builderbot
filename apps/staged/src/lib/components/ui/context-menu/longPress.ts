/**
 * Touch side of a context-menu long press.
 *
 * bits-ui opens a context menu after a 700 ms touch hold but leaves the rest
 * of the gesture alone: lifting the finger still fires `touchend` and the
 * browser's synthesized `click` on whatever is under it — the trigger's own
 * button or row, or the menu that just appeared — so the item activates (or a
 * menu entry fires) and the menu is unusable on mobile web. This controller
 * owns the touch gesture instead, and the trigger keeps bits-ui's timer from
 * starting (see context-menu-trigger.svelte), so a press declined or dropped
 * here opens nothing:
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
 *   `pointercancel` and on a `scroll` of a container holding the touched
 *   element that carries the element with it. A press that already fired
 *   keeps its suppression;
 * - a scroll that leaves the touched element where it is on screen keeps
 *   the press. That is scroll anchoring: when a row arrives or grows above
 *   the viewport (the timeline while a session runs), the browser moves the
 *   scroll position by the same amount and fires `scroll`, and the row under
 *   the finger stays put. The element's position is read at `touchstart` and
 *   again on each such scroll; one that cannot be measured counts as moved;
 * - a touched element replaced during the hold (a re-render, which the same
 *   timeline does to a live title) is in no scroller and has no position.
 *   The scroll test then follows the nearest element above it that is still
 *   in the DOM: every element from the touched one up to the trigger has its
 *   position read at `touchstart`. That is the held row or something in it.
 *   The trigger is not: one wraps a whole timeline, whose top an anchoring
 *   scroll moves while the row stays. With nothing left to follow, any
 *   scroll drops the press;
 * - a touch that lands on a list that is still moving is the user catching
 *   the scroll, not pressing a row: a `touchstart` within
 *   `SCROLL_CATCH_WINDOW_MS` of the last scroll of a container holding the
 *   touched element starts no press at all. A scroll-anchoring adjustment
 *   in that window counts as well: by then only the time of the scroll is
 *   on record, and telling the two apart would mean measuring every scroll
 *   in the app. The next touch presses.
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
/**
 * How far the touched element may sit from where it was at `touchstart`,
 * after a scroll, and still count as not carried by it. Scroll anchoring is
 * not exact: Safari keeps scroll positions in whole px, and a row held
 * through a 200 ms slide above it sat up to 1.25 px off in the iOS 27.2
 * Simulator. A scroll the finger started is past this in its first frames,
 * and announced by `pointercancel` before that.
 */
export const SCROLL_STILL_TOLERANCE_PX = 3;

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
   * opens for the element the user is holding. A scroll during the hold is
   * judged for such a press by what is left between that descendant and the
   * trigger.
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

/** Where an element's box starts on screen. */
interface ScreenPosition {
  left: number;
  top: number;
}

/**
 * Where `target` is on screen, or `null` when that cannot be told: a node
 * that left the DOM, or one with no box of its own.
 */
function positionOf(target: EventTarget): ScreenPosition | null {
  if (isDetached(target)) return null;
  const measure = (target as Partial<Element>).getBoundingClientRect;
  if (typeof measure !== 'function') return null;
  const { left, top, width, height } = measure.call(target);
  if (width === 0 && height === 0) return null;
  return { left, top };
}

/** An element at or above the touched one, and where it was at `touchstart`. */
interface Rested {
  node: EventTarget;
  at: ScreenPosition | null;
}

/**
 * The touched element and each element above it up to the trigger, nearest
 * first, with where each was on screen. With no trigger, the touched element
 * alone.
 */
function restingPlaces(target: EventTarget, trigger: EventTarget | null): Rested[] {
  const places: Rested[] = [];
  let node: EventTarget | null = target;
  while (node) {
    places.push({ node, at: positionOf(node) });
    node = trigger && node !== trigger ? ((node as Partial<Node>).parentNode ?? null) : null;
  }
  return places;
}

function apart(a: ScreenPosition, b: ScreenPosition): boolean {
  return (
    Math.abs(a.left - b.left) > SCROLL_STILL_TOLERANCE_PX ||
    Math.abs(a.top - b.top) > SCROLL_STILL_TOLERANCE_PX
  );
}

/** Recent scrolls seen on one document, shared by every controller on it. */
interface ScrollWatch {
  users: number;
  /**
   * When each container last scrolled. A timer drops an entry as soon as it
   * is older than the catch window, so no container is held on to once it
   * has stopped scrolling.
   */
  recent: Map<EventTarget | null, number>;
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
    let sweepTimer: ReturnType<typeof setTimeout> | null = null;
    // Drops what has aged out, and runs again when the oldest entry left is
    // due. One timer however many containers scroll, and none once the
    // record is empty.
    const sweep = () => {
      sweepTimer = null;
      const now = Date.now();
      let oldest = Infinity;
      for (const [scroller, at] of recent) {
        if (now - at > SCROLL_CATCH_WINDOW_MS) recent.delete(scroller);
        else oldest = Math.min(oldest, at);
      }
      if (recent.size > 0) {
        sweepTimer = setTimeout(sweep, oldest + SCROLL_CATCH_WINDOW_MS + 1 - now);
      }
    };
    const onScroll = (event: Event) => {
      recent.set(event.target, Date.now());
      sweepTimer ??= setTimeout(sweep, SCROLL_CATCH_WINDOW_MS + 1);
    };
    const created: ScrollWatch = {
      users: 0,
      recent,
      stop() {
        scope.removeEventListener('scroll', onScroll, opts);
        scrollWatches.delete(scope);
        if (sweepTimer !== null) clearTimeout(sweepTimer);
        sweepTimer = null;
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

/**
 * How many scrolled containers the watch on `scope` is holding on to. For
 * tests: the record has to empty itself once scrolling stops.
 */
export function recentScrollerCount(scope: EventTarget): number {
  return scrollWatches.get(scope)?.recent.size ?? 0;
}

/** Whether a container holding `target` scrolled within the catch window. */
function landedOnMovingList(watch: ScrollWatch, target: EventTarget): boolean {
  const now = Date.now();
  for (const [scroller, at] of watch.recent) {
    if (now - at <= SCROLL_CATCH_WINDOW_MS && scrollMoves(scroller, target)) return true;
  }
  return false;
}

export function createLongPress(options: LongPressOptions): LongPressController {
  const delay = options.delay ?? LONG_PRESS_DELAY_MS;
  const moveThreshold = options.moveThreshold ?? LONG_PRESS_MOVE_THRESHOLD_PX;

  let start: LongPressPoint | null = null;
  /** Where the touched element and those above it were when the finger landed. */
  let rested: Rested[] = [];
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
    rested = [];
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

  // A scroll that carries the touched element is the list moving under the
  // finger. One that leaves it in place is the browser holding the row still
  // while content above it changed height, and the press stands. With nothing
  // that can be measured, at `touchstart` or now, it counts as moved: dropping
  // a press costs a second try, keeping one opens a menu under a finger that
  // was scrolling.
  function onscroll(event: Event) {
    if (!start || timer === null) return;
    // A touched element replaced during the hold is inside no scroller and
    // has no position: the nearest element above it still in the DOM that had
    // a box at `touchstart` stands in for it. One with no box (a
    // `display: contents` wrapper) is passed over for the next one up, and
    // followed only when nothing above it had a box either.
    const present = rested.filter(({ node }) => !isDetached(node));
    const held = present.find(({ at }) => at) ?? present[0];
    if (!held) {
      clearTimer();
      return;
    }
    if (!scrollMoves(event.target, held.node)) return;
    const now = positionOf(held.node);
    if (!held.at || !now || apart(held.at, now)) clearTimer();
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
      rested = restingPlaces(target, options.trigger?.() ?? null);
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
