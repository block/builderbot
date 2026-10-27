import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  CLICK_SWALLOW_WINDOW_MS,
  LONG_PRESS_DELAY_MS,
  LONG_PRESS_MOVE_THRESHOLD_PX,
  createLongPress,
  type LongPressPoint,
} from './longPress';

type FakeTouch = { clientX: number; clientY: number; target: EventTarget };

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

// vitest runs in node: no MouseEvent constructor, and the controller only
// consults `isTrusted` and the two cancellation methods.
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
  // `trigger` stands in for the trigger element the swallow listens on; `row`
  // is the element under the finger inside it.
  let trigger: EventTarget;
  let onLongPress: ReturnType<typeof vi.fn<(point: LongPressPoint) => void>>;
  let row: EventTarget;

  beforeEach(() => {
    vi.useFakeTimers();
    trigger = new EventTarget();
    onLongPress = vi.fn<(point: LongPressPoint) => void>();
    row = new EventTarget();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function press(opts: { disabled?: boolean } = {}) {
    return createLongPress({
      onLongPress,
      trigger: () => trigger,
      isDisabled: () => opts.disabled ?? false,
    });
  }

  function clickPassesThrough(target: EventTarget = trigger): boolean {
    const seen = vi.fn();
    target.addEventListener('click', seen, { once: true });
    target.dispatchEvent(new Event('click', { cancelable: true }));
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

    const end = touchEvent([], row);
    lp.ontouchend(end);
    expect(end.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(false);
    // Only the one trailing click is swallowed.
    expect(clickPassesThrough()).toBe(true);
  });

  it('lets a short tap through untouched', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(200);
    const end = touchEvent([], row);
    lp.ontouchend(end);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    expect(end.defaultPrevented).toBe(false);
    expect(clickPassesThrough()).toBe(true);
  });

  it('tolerates jitter but cancels once the finger moves past the threshold', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    lp.ontouchmove(touchEvent([{ clientX: 3, clientY: -4, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    lp.ontouchend(touchEvent([], row));
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS);

    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    lp.ontouchmove(
      touchEvent([{ clientX: 0, clientY: LONG_PRESS_MOVE_THRESHOLD_PX + 1, target: row }])
    );
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    const end = touchEvent([], row);
    lp.ontouchend(end);
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
    lp.ontouchcancel(touchEvent([], row));
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
    const end = touchEvent([], row);
    lp.ontouchend(end);
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
    const end = touchEvent([{ clientX: 120, clientY: 80, target: row }], row);
    lp.ontouchend(end);
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
    const end = touchEvent([], row);
    lp.ontouchend(end);
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
    const end = touchEvent([], row);
    lp.ontouchend(end);
    expect(end.defaultPrevented).toBe(true);
  });

  it('ignores a mouse contextmenu with no touch down', () => {
    const lp = press();
    const event = contextMenuEvent({ trusted: true });
    expect(lp.oncontextmenu(event)).toBe(false);
    expect(event.defaultPrevented).toBe(false);
    const end = touchEvent([], row);
    lp.ontouchend(end);
    expect(end.defaultPrevented).toBe(false);
  });

  it('swallows a click on the trigger anywhere inside the window', () => {
    const lp = press();
    holdUntilFired(lp);
    lp.ontouchend(touchEvent([], row));
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS - 1);
    expect(clickPassesThrough(trigger)).toBe(false);
  });

  it('leaves a click outside the trigger alone inside the window', () => {
    const lp = press();
    holdUntilFired(lp);
    lp.ontouchend(touchEvent([], row));
    // A fast tap on a row of the menu that just opened (portalled elsewhere).
    const menuRow = new EventTarget();
    expect(clickPassesThrough(menuRow)).toBe(true);
    // ...and that did not spend the swallow meant for the trigger.
    expect(clickPassesThrough(trigger)).toBe(false);
  });

  it('defaults the swallow to the pressed element when no trigger is given', () => {
    const lp = createLongPress({ onLongPress });
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    lp.ontouchend(touchEvent([], row));
    expect(clickPassesThrough(row)).toBe(false);
  });

  it('stops swallowing clicks once the window expires', () => {
    const lp = press();
    holdUntilFired(lp);
    lp.ontouchend(touchEvent([], row));
    vi.advanceTimersByTime(CLICK_SWALLOW_WINDOW_MS);
    expect(clickPassesThrough()).toBe(true);
  });

  it('does nothing while disabled', () => {
    const lp = press({ disabled: true });
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(onLongPress).not.toHaveBeenCalled();
    const end = touchEvent([], row);
    lp.ontouchend(end);
    expect(end.defaultPrevented).toBe(false);
  });

  it('destroy drops a pending press and an armed click swallow', () => {
    const lp = press();
    holdUntilFired(lp);
    lp.ontouchend(touchEvent([], row));
    lp.destroy();
    expect(clickPassesThrough()).toBe(true);
  });
});
