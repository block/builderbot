import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  CLICK_SWALLOW_WINDOW_MS,
  LONG_PRESS_DELAY_MS,
  LONG_PRESS_MOVE_THRESHOLD_PX,
  createLongPress,
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
  // trigger wires it; the rest of the gesture is dispatched on `doc`.
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

  function gesture(type: 'touchmove' | 'touchend' | 'touchcancel', touches: FakeTouch[] = []) {
    const event = Object.assign(new Event(type, { cancelable: true }), { touches });
    doc.dispatchEvent(event);
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
    // The release is retargeted to the detached node by the browser and never
    // bubbles to the trigger; the document listener still suppresses it.
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(false);
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
    const end = gesture('touchend');
    expect(end.defaultPrevented).toBe(false);
  });
});
