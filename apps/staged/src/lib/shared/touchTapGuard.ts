// touchTapGuard.ts — ignore the click a flick ends in.
//
// iOS Safari turns a quick flick into a tap when it sees too few touch
// samples to start a scroll (60–200 px of travel, finger up within about
// 100 ms): no scroll happens, and a `click` lands on whatever was under the
// finger when it came down. On a list of cards that opens one. This is
// WebKit's tap synthesis, not an app handler, so it is undone in one place
// for every click target rather than per card.
//
// What tells such a touch from a tap is how far the finger went, in points on
// the screen, and where it went. Both limits are ones iOS already has, for a
// touch it has the samples to see move:
//
// - where nothing scrolls along the travel (a toolbar or dialog button, a
//   short menu, across a vertical list) iOS makes no click once the finger is
//   more than 45 pt from where it landed (measured in the iOS 27.2 Simulator:
//   44 px clicks, 48 px does not, quick or slow). A thumb rolls up to about
//   30 px as it presses, and iOS clicks for that
// - along a scroller iOS starts the scroll instead, and makes no click, once
//   the finger has gone about 10 pt (same Simulator, a project card in the
//   list: 8 px clicks, 12 px scrolls)
//
// A click after less travel than the limit for where it went is one iOS
// judged and meant. A click after more is one iOS made without seeing the
// travel, and those are the flicks: past 45 pt anywhere, and past 10 pt
// along something that scrolls. So a click is swallowed when the touch
// lifted
//
// - more than `TAP_MAX_TRAVEL_PX` away, or
// - more than `TAP_SCROLL_SLOP_PX` away along an axis on which an ancestor of
//   the element it landed on scrolls
//
// The second limit is held to each axis's own part of the travel, not to the
// larger part alone. A flick 26 px across and 24 px up a vertical list has
// gone 24 pt along the list, and is swallowed: given the samples, iOS makes no
// click for that stroke (same Simulator, a project card). A roll 26 px across
// the list with 8 px of drift along it has not, and passes.
//
// "Scrolls along an axis" is `scrollsAlong` below: content that overflows on
// that axis in a box the finger can scroll, wherever it is scrolled to. The
// search starts at the element the touch landed on, or, where a re-render
// took that element out of the document during the touch, at the element now
// at the point it landed on.
//
// How long the touch lasted is not asked. A quick tap is as short as a
// flick, and a slow touch that went far and still ends in a click is no more
// a tap than a quick one.
//
// Distance is measured with pointer events. For such a flick `touchend`
// reports the position the touch started at, while `pointerup` reports where
// the finger actually lifted. Their positions are in layout px, and a
// pinch-zoomed page shows each as `visualViewport.scale` points, so the
// distance is multiplied by the scale before it is held to either limit.

/**
 * Travel between finger down and finger up, in screen points, up to which a
 * touch is a tap wherever it landed and whatever its duration: the limit iOS
 * itself applies to a touch it has seen move. It leaves half as much again
 * above the furthest roll of a pressing thumb (about 30 px), and the shortest
 * flick the audit saw turned into a tap (60 px) is a third beyond it.
 */
export const TAP_MAX_TRAVEL_PX = 45;

/**
 * Travel, in screen points, up to which a touch is a tap even along
 * something that scrolls: the distance iOS lets a finger go before it starts
 * the scroll.
 */
export const TAP_SCROLL_SLOP_PX = 10;

/** How long after a travelled touch ends its click can still arrive. */
export const TAP_CLICK_WINDOW_MS = 700;

/** The parts of a pointer event the guard reads. */
type PointerLike = Pick<
  PointerEvent,
  'pointerId' | 'pointerType' | 'isPrimary' | 'clientX' | 'clientY' | 'timeStamp' | 'target'
>;

/** The parts of a click event the guard reads and acts on. */
type ClickLike = Pick<
  MouseEvent,
  'isTrusted' | 'timeStamp' | 'preventDefault' | 'stopImmediatePropagation'
>;

type Axis = 'x' | 'y';

/** What the guard reads of an element between the touched one and the document. */
type ElementLike = Pick<
  Element,
  | 'parentElement'
  | 'ownerDocument'
  | 'scrollWidth'
  | 'scrollHeight'
  | 'clientWidth'
  | 'clientHeight'
>;

/** A touch being followed: its pointer, and where and on what it landed. */
interface Press {
  pointerId: number;
  x: number;
  y: number;
  target: EventTarget | null;
}

/** How many screen points one layout px covers: above 1 while pinch-zoomed. */
function pointsPerPx(): number {
  const scale = typeof visualViewport === 'undefined' ? 1 : visualViewport?.scale;
  return typeof scale === 'number' && scale > 0 ? scale : 1;
}

function styleOf(element: ElementLike): CSSStyleDeclaration | null {
  return typeof getComputedStyle === 'undefined' ? null : getComputedStyle(element as Element);
}

/** Whether the element's content is larger than its box along the axis. */
function overflows(element: ElementLike, axis: Axis): boolean {
  return axis === 'y'
    ? element.scrollHeight > element.clientHeight
    : element.scrollWidth > element.clientWidth;
}

/** Whether `touch-action` leaves the browser free to pan along the axis. */
function allowsPan(touchAction: string, axis: Axis): boolean {
  if (!touchAction || touchAction === 'auto' || touchAction === 'manipulation') return true;
  const parts = touchAction.split(/\s+/);
  return axis === 'y'
    ? parts.some((part) => part === 'pan-y' || part === 'pan-up' || part === 'pan-down')
    : parts.some((part) => part === 'pan-x' || part === 'pan-left' || part === 'pan-right');
}

/**
 * Whether a finger can scroll the document along the axis. The viewport
 * takes its `overflow` from <html>, or from <body> where <html> leaves it
 * `visible`, and scrolls unless that is `hidden` or `clip`. The app sets
 * both to `hidden`, so its document never counts; the test is here for a
 * page that leaves it open.
 */
function viewportScrolls(doc: Document, axis: Axis): boolean {
  const html = doc.documentElement as ElementLike | null;
  if (!html) return false;
  const property = axis === 'y' ? 'overflowY' : 'overflowX';
  let overflow = styleOf(html)?.[property] ?? 'visible';
  if (overflow === 'visible' && doc.body) overflow = styleOf(doc.body)?.[property] ?? 'visible';
  if (overflow === 'hidden' || overflow === 'clip') return false;
  return overflows((doc.scrollingElement as ElementLike | null) ?? html, axis);
}

/**
 * The element to look for a scroller from: the one the touch landed on, or,
 * when that has left the document since (a live title re-rendered under the
 * finger, say), the one now at the point it landed on. The click goes to
 * what is there now, and so would the scroll have. `null` when there is no
 * such element.
 */
function landingOf(press: Press): ElementLike | null {
  const first = press.target as (Partial<Element> & ElementLike) | null;
  // A text node cannot be a pointer target, but a stand-in or a document can.
  if (!first || first.nodeType !== 1) return null;
  if (first.isConnected !== false) return first;
  return first.ownerDocument?.elementFromPoint?.(press.x, press.y) ?? null;
}

/**
 * Whether `from`, the element a touch landed on, sits in something a finger
 * can scroll along the axis:
 *
 * - an ancestor (or the element) whose `overflow` on that axis is `auto` or
 *   `scroll` and whose content is larger than its box on that axis. One
 *   with nothing to scroll does not count, whatever its `overflow`, and
 *   neither does `overflow: hidden`, which a finger cannot scroll
 * - where it is scrolled to is not asked. At its end in the direction of
 *   travel iOS still takes the touch, to rubber-band or to scroll what is
 *   outside it, and makes no click (the project list at either end, 24 px,
 *   iOS 27.2 Simulator)
 * - `touch-action` on the way up that rules out panning along the axis ends
 *   the search: the browser will not scroll for a touch that starts there
 * - past <body>, the document itself: see `viewportScrolls`
 *
 * The page a pinch-zoom or the on-screen keyboard lets the finger pan is not
 * counted.
 */
function scrollsAlong(from: ElementLike | null, axis: Axis): boolean {
  const doc = from?.ownerDocument;
  if (!doc) return false;
  for (let element: ElementLike | null = from; element; element = element.parentElement) {
    const style = styleOf(element);
    if (!style) return false;
    if (!allowsPan(style.touchAction, axis)) return false;
    if (element === doc.body || element === doc.documentElement) return viewportScrolls(doc, axis);
    const overflow = axis === 'y' ? style.overflowY : style.overflowX;
    if ((overflow === 'auto' || overflow === 'scroll') && overflows(element, axis)) return true;
  }
  // The walk did not reach the document: nothing to scroll.
  return false;
}

/**
 * Swallow the click that follows a primary touch lifted further from where
 * it landed than a tap goes: more than `TAP_MAX_TRAVEL_PX` anywhere, or more
 * than `TAP_SCROLL_SLOP_PX` along an axis something there scrolls on, however
 * long it lasted. Mouse and pen input, taps, rolled presses and scripted
 * clicks pass through untouched. Returns the cleanup.
 */
export function guardTouchTaps(target?: EventTarget): () => void {
  const root = target ?? (typeof document === 'undefined' ? undefined : document);
  if (!root) return () => {};

  /** The touch being followed, from finger down to finger up. */
  let press: Press | null = null;
  /** `timeStamp` of a travelled touch's `pointerup` while its click is awaited. */
  let travelledAt: number | null = null;

  const onPointerDown = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    // Any new press starts over: a click still owed to the last touch never came.
    travelledAt = null;
    press =
      pointer.pointerType === 'touch' && pointer.isPrimary
        ? {
            pointerId: pointer.pointerId,
            x: pointer.clientX,
            y: pointer.clientY,
            target: pointer.target,
          }
        : null;
  };

  const onPointerUp = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    if (!press || pointer.pointerId !== press.pointerId) return;
    const dx = pointer.clientX - press.x;
    const dy = pointer.clientY - press.y;
    const landed = press;
    press = null;
    const scale = pointsPerPx();
    const travel = Math.hypot(dx, dy) * scale;
    if (travel > TAP_MAX_TRAVEL_PX) {
      travelledAt = pointer.timeStamp;
      return;
    }
    // Each axis the finger went further along than a scroll takes to start.
    const axes: Axis[] = [];
    if (Math.abs(dy) * scale > TAP_SCROLL_SLOP_PX) axes.push('y');
    if (Math.abs(dx) * scale > TAP_SCROLL_SLOP_PX) axes.push('x');
    // The DOM is read only for a touch between the two limits.
    if (axes.length === 0) return;
    const from = landingOf(landed);
    if (axes.some((axis) => scrollsAlong(from, axis))) travelledAt = pointer.timeStamp;
  };

  // The browser took the gesture (a scroll, usually) and sends no click.
  const onPointerCancel = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    if (press && pointer.pointerId === press.pointerId) press = null;
  };

  const onClick = (event: Event) => {
    if (travelledAt === null) return;
    const click = event as unknown as ClickLike;
    // A click made by script (`element.click()`) is not the touch's own.
    if (!click.isTrusted) return;
    const elapsed = click.timeStamp - travelledAt;
    travelledAt = null;
    if (elapsed > TAP_CLICK_WINDOW_MS) return;
    click.preventDefault();
    click.stopImmediatePropagation();
  };

  // Capture phase: ahead of every handler in the app, and unaffected by
  // components that stop propagation.
  root.addEventListener('pointerdown', onPointerDown, true);
  root.addEventListener('pointerup', onPointerUp, true);
  root.addEventListener('pointercancel', onPointerCancel, true);
  root.addEventListener('click', onClick, true);
  return () => {
    root.removeEventListener('pointerdown', onPointerDown, true);
    root.removeEventListener('pointerup', onPointerUp, true);
    root.removeEventListener('pointercancel', onPointerCancel, true);
    root.removeEventListener('click', onClick, true);
  };
}
