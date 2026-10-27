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
// consults the event's existence.
const contextMenuEvent = () => ({}) as MouseEvent;

describe('createLongPress', () => {
  let clickTarget: EventTarget;
  let onLongPress: ReturnType<typeof vi.fn<(point: LongPressPoint) => void>>;
  let row: EventTarget;

  beforeEach(() => {
    vi.useFakeTimers();
    clickTarget = new EventTarget();
    onLongPress = vi.fn<(point: LongPressPoint) => void>();
    row = new EventTarget();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function press(opts: { disabled?: boolean } = {}) {
    return createLongPress({
      onLongPress,
      clickTarget,
      isDisabled: () => opts.disabled ?? false,
    });
  }

  function clickPassesThrough(): boolean {
    const seen = vi.fn();
    clickTarget.addEventListener('click', seen, { once: true });
    clickTarget.dispatchEvent(new Event('click', { cancelable: true }));
    return seen.mock.calls.length === 1;
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

  it("treats the browser's own contextmenu during a touch as a fired press", () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(100);
    lp.oncontextmenu(contextMenuEvent());
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    // The browser opened the menu; our timer must not open it a second time.
    expect(onLongPress).not.toHaveBeenCalled();
    const end = touchEvent([], row);
    lp.ontouchend(end);
    expect(end.defaultPrevented).toBe(true);
    expect(clickPassesThrough()).toBe(false);
  });

  it('ignores a mouse contextmenu with no touch down', () => {
    const lp = press();
    lp.oncontextmenu(contextMenuEvent());
    const end = touchEvent([], row);
    lp.ontouchend(end);
    expect(end.defaultPrevented).toBe(false);
  });

  it('stops swallowing clicks once the window expires', () => {
    const lp = press();
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
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
    lp.ontouchstart(touchEvent([{ clientX: 0, clientY: 0, target: row }]));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    lp.ontouchend(touchEvent([], row));
    lp.destroy();
    expect(clickPassesThrough()).toBe(true);
  });
});
