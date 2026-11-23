import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  CLICK_SWALLOW_WINDOW_MS,
  LONG_PRESS_DELAY_MS,
  LONG_PRESS_MOVE_THRESHOLD_PX,
  SCROLL_CATCH_WINDOW_MS,
  SCROLL_STILL_TOLERANCE_PX,
  createLongPress,
  recentScrollerCount,
  type LongPressPoint,
} from './longPress';

type FakeTouch = { clientX: number; clientY: number; target: EventTarget };

// vitest runs in node: no Touch/MouseEvent constructors and no `document`.
// The controller only reads `touches`, `target` and the cancellation
// methods, and takes the document it listens on as an option.
function touchEvent(touches: FakeTouch[], target: EventTarget = touches[0]?.target) {
  const event = {
    touches,
    target,
    defaultPrevented: false,
    preventDefault() {
      this.defaultPrevented = true;
    },
  };
  return event as unknown as TouchEvent & { defaultPrevented: boolean };
}

function contextMenuEvent({ trusted = false } = {}) {
  const event = {
    isTrusted: trusted,
    defaultPrevented: false,
    propagationStopped: false,
    preventDefault() {
      this.defaultPrevented = true;
    },
    stopImmediatePropagation() {
      this.propagationStopped = true;
    },
  };
  return event as unknown as MouseEvent & { propagationStopped: boolean };
}

describe('createLongPress', () => {
  // `doc` stands in for the document the gesture-wide listeners live on;
  // `trigger` is the trigger element and `row` the element under the finger
  // inside it. Only touchstart reaches the controller directly, the way the
  // trigger wires it; the rest of the gesture is dispatched on `doc` (where
  // a connected node's events arrive first), or on the node itself for the
  // detached case.
  let doc: EventTarget;
  let trigger: EventTarget;
  let row: EventTarget;
  let onLongPress: ReturnType<typeof vi.fn<(point: LongPressPoint) => void>>;

  beforeEach(() => {
    vi.useFakeTimers();
    doc = new EventTarget();
    trigger = new EventTarget();
    row = new EventTarget();
    onLongPress = vi.fn<(point: LongPressPoint) => void>();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function press(opts: { disabled?: boolean } = {}) {
    return createLongPress({
      onLongPress,
      trigger: () => trigger,
      scope: () => doc,
      isDisabled: () => opts.disabled ?? false,
    });
  }

  function gesture(
    type: 'touchstart' | 'touchmove' | 'touchend' | 'touchcancel',
    touches: FakeTouch[] = [],
    on: EventTarget = doc
  ) {
    const event = Object.assign(new Event(type, { cancelable: true }), { touches });
    on.dispatchEvent(event);
    return event;
  }

  // A click anywhere in the document; the swallow is scoped to the document,
  // not to the trigger, so the target only matters for the listener below.
  function clickPassesThrough(): boolean {
    const seen = vi.fn();
    doc.addEventListener('click', seen, { once: true });
    doc.dispatchEvent(new Event('click', { cancelable: true }));
    return seen.mock.calls.length === 1;
  }

  function holdUntilFired(lp: ReturnType<typeof press>) {
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 80, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  }

  it('fires after the delay and suppresses the release and its click', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 80, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS - 1);
    expect(onLongPress).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onLongPress).toHaveBeenCalledWith({ target: row, clientX: 40, clientY: 80 });

    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(false);
    // Only the one trailing click is swallowed.
    expect(clickPassesThrough()).toBe(true);
  });

  it('lets a short tap through untouched', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(200);
    const end = gesture('touchend');
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    expect(end.defaultPrevented).toBe(false);
    expect(clickPassesThrough()).toBe(true);
  });

  it('tolerates jitter but cancels once the finger moves past the threshold', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    gesture('touchmove', [{ clientX: 3, clientY: -4, target: row }]);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    gesture('touchend');
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS);

    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    gesture('touchmove', [{ clientX: 0, clientY: LONG_PRESS_MOVE_THRESHOLD_PX + 1, target: row }]);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(false);
  });

  it('cancels on a second finger or touchcancel', () => {
    const lp = press();
    lp.ontouchstart(
      touchEvent([
        { clientX: 0, clientY: 0, target: row },
        { clientX: 50, clientY: 0, target: row },
      ])
    );
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();

    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    gesture('touchcancel');
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('drops a pending press when a second finger lands mid-hold', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(100);
    lp.ontouchstart(
      touchEvent([
        { clientX: 0, clientY: 0, target: row },
        { clientX: 50, clientY: 0, target: row },
      ])
    );
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(false);
  });

  it('keeps suppressing the release when a second finger lands after the press fired', () => {
    const lp = press();
    holdUntilFired(lp);
    // Menu is open, first finger still down, a second one touches the screen.
    lp.ontouchstart(
      touchEvent([
        { clientX: 40, clientY: 80, target: row },
        { clientX: 120, clientY: 80, target: row },
      ])
    );
    const end = gesture('touchend', [{ clientX: 120, clientY: 80, target: row }]);
    expect(end.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(false);
  });

  it("treats the browser's own contextmenu during a touch as a fired press", () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(100);
    const native = contextMenuEvent({ trusted: true });
    expect(lp.oncontextmenu(native)).toBe(false);
    // The browser's event is the one opening the menu; it must go through.
    expect(native.defaultPrevented).toBe(false);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    // The browser opened the menu; our timer must not open it a second time.
    expect(onLongPress).not.toHaveBeenCalled();
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(false);
  });

  it('eats a trusted contextmenu that trails our synthetic one', () => {
    const lp = press();
    holdUntilFired(lp);
    // The trigger re-dispatches the press as a synthetic (untrusted) event.
    const synthetic = contextMenuEvent();
    expect(lp.oncontextmenu(synthetic)).toBe(false);
    expect(synthetic.defaultPrevented).toBe(false);
    // Android: the browser's own contextmenu for the same hold lands next.
    const native = contextMenuEvent({ trusted: true });
    expect(lp.oncontextmenu(native)).toBe(true);
    expect(native.defaultPrevented).toBe(true);
    expect(native.propagationStopped).toBe(true);
    // The release is still suppressed as for any fired press.
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(true);
  });

  it('ignores a mouse contextmenu with no touch down', () => {
    const lp = press();
    const event = contextMenuEvent({ trusted: true });
    expect(lp.oncontextmenu(event)).toBe(false);
    expect(event.defaultPrevented).toBe(false);
    // No press is live, so nothing is listening for a release either.
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(false);
  });

  it('swallows a click anywhere in the document inside the window', () => {
    const lp = press();
    holdUntilFired(lp);
    gesture('touchend');
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS - 1);
    // Whatever the synthesized click hit-tests onto — the trigger's row or the
    // sheet layer now covering it — the capture listener sees it first.
    const seen = vi.fn();
    row.addEventListener('click', seen);
    const click = new Event('click', { cancelable: true });
    doc.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(true);
  });

  it('stops swallowing clicks once the window expires', () => {
    const lp = press();
    holdUntilFired(lp);
    gesture('touchend');
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS);
    expect(clickPassesThrough()).toBe(true);
  });

  it('lets a click from a new touch through, however fast it follows the release', () => {
    const lp = press();
    holdUntilFired(lp);
    gesture('touchend');
    // The user's next tap (on a row of the sheet that just opened) begins
    // inside the window: its touchstart marks the coming click as legitimate.
    doc.dispatchEvent(new Event('touchstart'));
    expect(clickPassesThrough()).toBe(true);
  });

  it('fires at the trigger when the pressed element left the DOM during the hold', () => {
    const lp = press();
    const detached = Object.assign(new EventTarget(), { isConnected: false });
    lp.ontouchstart(touchEvent([{ clientX: 10, clientY: 20, target: detached }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledWith({ target: trigger, clientX: 10, clientY: 20 });
    // Blink dispatches the release at the stored touchstart target and walks
    // its (now empty) ancestry for the path: the detached node is the only
    // place it can be seen. Nothing reaches the document.
    const end = gesture('touchend', [], detached);
    expect(end.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(false);
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS);
    // The listeners came down with the press: a later event on the detached
    // node, or on the document, is nobody's release.
    expect(gesture('touchend', [], detached).defaultPrevented).toBe(false);
    expect(gesture('touchend').defaultPrevented).toBe(false);
    // The next tap on some other element is an ordinary tap.
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(100);
    expect(gesture('touchend').defaultPrevented).toBe(false);
    expect(clickPassesThrough()).toBe(true);
  });

  it('drops a stale press when a new single touch starts anywhere in the document', () => {
    const lp = press();
    // The press fired but its release was never delivered to us (its target
    // was detached and the browser handed the touchend to no one else).
    holdUntilFired(lp);
    // The user's next tap lands on a row of the sheet that just opened: the
    // document sees its touchstart before anything else does.
    gesture('touchstart', [{ clientX: 100, clientY: 600, target: doc }]);
    // That tap's release and click are left alone.
    expect(gesture('touchend').defaultPrevented).toBe(false);
    expect(clickPassesThrough()).toBe(true);
    // And a trusted contextmenu afterwards is not mistaken for the hold's duplicate.
    const native = contextMenuEvent({ trusted: true });
    expect(lp.oncontextmenu(native)).toBe(false);
    expect(native.defaultPrevented).toBe(false);
  });

  it('drops a pending press when a second finger lands outside the trigger', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(100);
    // Only the document sees a second finger that lands off the trigger.
    gesture('touchstart', [
      { clientX: 0, clientY: 0, target: row },
      { clientX: 200, clientY: 0, target: doc },
    ]);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    expect(gesture('touchend').defaultPrevented).toBe(false);
  });

  // A `scroll` as the document's capture listener sees it: `target` is the
  // container that scrolled. `list` holds `row`; `elsewhere` does not.
  function scroll(scroller: EventTarget = doc) {
    const event = new Event('scroll');
    Object.defineProperty(event, 'target', { value: scroller });
    doc.dispatchEvent(event);
  }

  function scroller(holds: EventTarget[]) {
    return Object.assign(new EventTarget(), {
      contains: (node: EventTarget) => holds.includes(node),
    });
  }

  function pointerCancel(pointerType = 'touch', on: EventTarget = doc) {
    on.dispatchEvent(Object.assign(new Event('pointercancel'), { pointerType }));
  }

  it('cancels a pending press on pointercancel, the way iOS announces a scroll', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    // A slow drag: exactly the threshold, so the move test alone keeps the press.
    gesture('touchmove', [{ clientX: 0, clientY: LONG_PRESS_MOVE_THRESHOLD_PX, target: row }]);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS - 50);
    pointerCancel();
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    // No touchcancel follows on iOS; the release is an ordinary one.
    expect(gesture('touchend').defaultPrevented).toBe(false);
    expect(clickPassesThrough()).toBe(true);
  });

  it("ignores another input's pointercancel", () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    pointerCancel('mouse');
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('sees the pointercancel of a pressed element that left the DOM', () => {
    const lp = press();
    const detached = Object.assign(new EventTarget(), { isConnected: false });
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: detached }]));
    pointerCancel('touch', detached);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('keeps suppressing the release when pointercancel or a scroll follows a fired press', () => {
    const lp = press();
    holdUntilFired(lp);
    pointerCancel();
    scroll();
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(false);
  });

  it('cancels a pending press when the list under the finger scrolls', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS - 50);
    scroll(scroller([row]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    expect(gesture('touchend').defaultPrevented).toBe(false);

    // The document scrolling moves every row.
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(SCROLL_CATCH_WINDOW_MS + 1);
    scroll();
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('keeps a pending press when an unrelated pane scrolls', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    scroll(scroller([]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  // An element with a box on screen, which `at` moves.
  function boxed(at: { left: number; top: number }) {
    return Object.assign(new EventTarget(), {
      isConnected: true,
      getBoundingClientRect: () => ({ left: at.left, top: at.top, width: 320, height: 44 }),
    });
  }

  it('keeps a pending press through scrolls that leave the touched element in place', () => {
    const lp = press();
    const at = { left: 16, top: 300 };
    const held = boxed(at);
    const list = scroller([held]);
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    // A row sliding in above the viewport: the browser moves the scroll
    // position every frame to hold the rows in view where they are.
    for (let frame = 0; frame < 12; frame += 1) {
      vi.advanceTimersByTime(16);
      at.top = 300 + (frame % 2 === 0 ? 0.33 : -0.33);
      scroll(list);
    }
    // The document scrolling without moving the element is no different.
    scroll();
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(onLongPress).toHaveBeenCalledWith({ target: held, clientX: 40, clientY: 320 });
    expect(gesture('touchend').defaultPrevented).toBe(true);
  });

  it('cancels a pending press once a scroll has carried the touched element', () => {
    const lp = press();
    const at = { left: 16, top: 300 };
    const held = boxed(at);
    const list = scroller([held]);

    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    at.top = 300 - SCROLL_STILL_TOLERANCE_PX;
    scroll(list);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    gesture('touchend');
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS + SCROLL_CATCH_WINDOW_MS);

    // A slow scroll start: each frame is inside the tolerance, and the
    // distance is taken from where the element was when the finger landed.
    at.top = 300;
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    at.top = 300 - SCROLL_STILL_TOLERANCE_PX + 0.5;
    scroll(list);
    at.top = 300 - SCROLL_STILL_TOLERANCE_PX - 1;
    scroll(list);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(gesture('touchend').defaultPrevented).toBe(false);
    vi.advanceTimersByTime(SCROLL_CATCH_WINDOW_MS + 1);

    // Sideways counts too (the board's columns).
    at.top = 300;
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    at.left = 16 + SCROLL_STILL_TOLERANCE_PX + 1;
    scroll(list);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('cancels on a scroll when the touched element cannot be placed', () => {
    const lp = press();

    // No box of its own, at the touch or at the scroll.
    const boxless = Object.assign(new EventTarget(), {
      isConnected: true,
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 0, height: 0 }),
    });
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: boxless }]));
    scroll(scroller([boxless]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    gesture('touchend');
    vi.advanceTimersByTime(SCROLL_CATCH_WINDOW_MS + 1);

    // Replaced by a re-render during the hold, in a trigger that cannot be
    // placed either: where the element was says nothing about where the row
    // is now.
    const replaced = boxed({ left: 16, top: 300 });
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: replaced }]));
    replaced.isConnected = false;
    scroll(scroller([trigger]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  // A `display: contents` trigger, as the app's are: no box of its own, and
  // `held` inside it. `replace()` is a keyed re-render of what it holds: the
  // touched element leaves the DOM and a new one takes its place, at `at`.
  function contentsTrigger(held: ReturnType<typeof boxed>, at: { left: number; top: number }) {
    const wrapper = Object.assign(new EventTarget(), {
      isConnected: true,
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 0, height: 0 }),
      children: [held],
      replace() {
        held.isConnected = false;
        wrapper.children = [boxed(at)];
      },
    });
    return wrapper;
  }

  it('follows the trigger through a scroll once the touched element was replaced', () => {
    const lp = press();
    const at = { left: 16, top: 300 };
    const held = boxed(at);
    const wrapper = contentsTrigger(held, at);
    trigger = wrapper;
    // A real scroller holds the trigger and, once it is replaced, not the
    // element the touch started on.
    const list = scroller([wrapper]);

    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    vi.advanceTimersByTime(100);
    wrapper.replace();
    // Scroll anchoring for the row that arrived above: the trigger stays put.
    for (let frame = 0; frame < 12; frame += 1) {
      vi.advanceTimersByTime(16);
      at.top = 300 + (frame % 2 === 0 ? 0.33 : -0.33);
      scroll(list);
    }
    scroll();
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(onLongPress).toHaveBeenCalledWith({ target: wrapper, clientX: 40, clientY: 320 });
    // The release is delivered to the replaced element alone.
    expect(gesture('touchend', [], held).defaultPrevented).toBe(true);
  });

  it('cancels once a scroll has carried the trigger of a replaced element', () => {
    const lp = press();
    const at = { left: 16, top: 300 };
    const held = boxed(at);
    const wrapper = contentsTrigger(held, at);
    trigger = wrapper;

    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    vi.advanceTimersByTime(100);
    wrapper.replace();
    at.top = 300 - SCROLL_STILL_TOLERANCE_PX - 1;
    scroll(scroller([wrapper]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    expect(gesture('touchend', [], held).defaultPrevented).toBe(false);
  });

  it('measures the trigger from where the finger landed, not from the replacement', () => {
    const lp = press();
    const at = { left: 16, top: 300 };
    const held = boxed(at);
    const wrapper = contentsTrigger(held, at);
    trigger = wrapper;
    const list = scroller([wrapper]);

    // A slow scroll start: each frame is inside the tolerance of the last.
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    at.top = 300 - SCROLL_STILL_TOLERANCE_PX + 0.5;
    scroll(list);
    wrapper.replace();
    at.top = 300 - SCROLL_STILL_TOLERANCE_PX - 1;
    scroll(list);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('follows a trigger with a box of its own', () => {
    const lp = press();
    const at = { left: 16, top: 300 };
    const held = boxed({ left: 24, top: 310 });
    const own = boxed(at);
    trigger = own;
    const list = scroller([own]);

    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    held.isConnected = false;
    scroll(list);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledWith({ target: own, clientX: 40, clientY: 320 });
    gesture('touchend', [], held);
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS + SCROLL_CATCH_WINDOW_MS);

    const again = boxed({ left: 24, top: 310 });
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: again }]));
    again.isConnected = false;
    at.left = 16 + SCROLL_STILL_TOLERANCE_PX + 1;
    scroll(list);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('keeps a replaced element’s press when an unrelated pane scrolls', () => {
    const lp = press();
    const at = { left: 16, top: 300 };
    const held = boxed(at);
    const wrapper = contentsTrigger(held, at);
    trigger = wrapper;

    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    wrapper.replace();
    scroll(scroller([]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('cancels on any scroll when a replaced element has no trigger left to follow', () => {
    const at = { left: 16, top: 300 };

    // The trigger left the DOM as well.
    const lp = press();
    const held = boxed(at);
    const wrapper = contentsTrigger(held, at);
    trigger = wrapper;
    lp.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: held }]));
    wrapper.replace();
    wrapper.isConnected = false;
    scroll(scroller([]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    gesture('touchend', [], held);
    lp.destroy();

    // No trigger was given.
    const bare = createLongPress({ onLongPress, scope: () => doc });
    const alone = boxed(at);
    bare.ontouchstart(touchEvent([{ clientX: 40, clientY: 320, target: alone }]));
    alone.isConnected = false;
    scroll(scroller([]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('starts no press for a touch that catches a moving list', () => {
    const lp = press();
    const list = scroller([row]);
    scroll(list);
    vi.advanceTimersByTime(SCROLL_CATCH_WINDOW_MS);
    // The finger lands while the list is still coasting and stays down.
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS * 2);
    expect(onLongPress).not.toHaveBeenCalled();
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(false);
    expect(clickPassesThrough()).toBe(true);
    // A browser contextmenu for that hold is not ours to act on either.
    const native = contextMenuEvent({ trusted: true });
    expect(lp.oncontextmenu(native)).toBe(false);
    expect(native.defaultPrevented).toBe(false);
  });

  it('presses normally once the list has come to rest, or when another pane is moving', () => {
    const lp = press();
    scroll(scroller([row]));
    vi.advanceTimersByTime(SCROLL_CATCH_WINDOW_MS + 1);
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    gesture('touchend');
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS);

    scroll(scroller([]));
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(2);
  });

  it('shares one scroll listener per document and drops it with the last controller', () => {
    const add = vi.spyOn(doc, 'addEventListener');
    const remove = vi.spyOn(doc, 'removeEventListener');
    const scrollCalls = (spy: typeof add) => spy.mock.calls.filter(([type]) => type === 'scroll');
    const first = press();
    const second = press();
    expect(scrollCalls(add)).toHaveLength(1);
    first.destroy();
    // A repeated destroy must not release the other controller's share.
    first.destroy();
    expect(scrollCalls(remove)).toHaveLength(0);
    scroll(scroller([row]));
    second.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    second.destroy();
    expect(scrollCalls(remove)).toHaveLength(1);
  });

  it('forgets each container once it has stopped scrolling', () => {
    const lp = press();
    const transcript = scroller([]);
    const dialogBody = scroller([]);
    scroll(transcript);
    scroll(dialogBody);
    scroll();
    expect(recentScrollerCount(doc)).toBe(3);

    // A transcript following its stream never lets the record go quiet. The
    // others leave it all the same.
    for (let elapsed = 0; elapsed <= SCROLL_CATCH_WINDOW_MS; elapsed += 50) {
      vi.advanceTimersByTime(50);
      scroll(transcript);
    }
    expect(recentScrollerCount(doc)).toBe(1);

    // And the last one goes with no further scroll to push it out.
    vi.advanceTimersByTime(SCROLL_CATCH_WINDOW_MS + 1);
    expect(recentScrollerCount(doc)).toBe(0);
    expect(vi.getTimerCount()).toBe(0);

    // The record still works afterwards.
    scroll(scroller([row]));
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('drops the scroll record and its timer with the last controller', () => {
    const lp = press();
    scroll(scroller([row]));
    expect(recentScrollerCount(doc)).toBe(1);
    lp.destroy();
    expect(recentScrollerCount(doc)).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('does nothing while disabled', () => {
    const lp = press({ disabled: true });
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(false);
  });

  it('stops listening for the gesture once it ends', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    gesture('touchend');
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS);
    // A later release belonging to some other gesture is none of ours.
    holdUntilFired(lp);
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(true);
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS);
    const stray = gesture('touchend');
    expect(stray.defaultPrevented).toBe(false);
  });

  it('destroy drops a pending press, the gesture listeners and an armed click swallow', () => {
    const lp = press();
    holdUntilFired(lp);
    gesture('touchend');
    lp.destroy();
    expect(clickPassesThrough()).toBe(true);
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    lp.destroy();
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(gesture('touchend').defaultPrevented).toBe(false);
    // The touched node's own listeners go too.
    expect(gesture('touchend', [], row).defaultPrevented).toBe(false);
  });
});
