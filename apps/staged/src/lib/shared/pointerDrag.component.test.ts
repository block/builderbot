import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  POINTER_DRAG_ATTRIBUTE,
  pointerDrag,
  type PointerDragOptions,
  type PointerDragPoint,
} from './pointerDrag';

// jsdom is only provided to `*.component.test.ts` files (see vitest.config.ts).
// The action is a plain function over a node, so the tests call it as Svelte
// would: once with the node, then `update` and `destroy`.

/** What the surface heard, in order. */
let calls: string[];
/** What the surface's `move` answers. */
let accepts: boolean;
let handle: HTMLButtonElement;
let label: HTMLSpanElement;
let outside: HTMLDivElement;
let clicks: ReturnType<typeof vi.fn<(event: MouseEvent) => void>>;
let action: ReturnType<typeof pointerDrag>;

/** The frames asked for and not yet run or cancelled. */
let frames: Map<number, FrameRequestCallback>;
let nextFrameId: number;

const xy = (point: PointerDragPoint) => `${point.clientX},${point.clientY}`;

function surface(overrides: Partial<PointerDragOptions> = {}): PointerDragOptions {
  return {
    start: (pressed) => calls.push(`start ${xy(pressed)}`),
    move: (point) => {
      calls.push(`move ${xy(point)}`);
      return accepts;
    },
    drop: (point) => calls.push(`drop ${xy(point)}`),
    cancel: () => calls.push('cancel'),
    ...overrides,
  };
}

function attach(overrides: Partial<PointerDragOptions> = {}) {
  action = pointerDrag(handle, surface(overrides));
}

/** A mouse's primary button, as the engines send it. The release has no button down. */
function pointer(
  type: string,
  target: EventTarget,
  clientX: number,
  clientY: number,
  init: PointerEventInit = {}
): PointerEvent {
  const event = new PointerEvent(type, {
    bubbles: true,
    cancelable: true,
    pointerId: 1,
    pointerType: 'mouse',
    isPrimary: true,
    button: 0,
    buttons: type === 'pointerup' || type === 'pointercancel' ? 0 : 1,
    clientX,
    clientY,
    ...init,
  });
  target.dispatchEvent(event);
  return event;
}

const press = (x = 100, y = 100, init: PointerEventInit = {}) =>
  pointer('pointerdown', handle, x, y, init);
/** Moves and releases go to whatever is under the pointer: here, not the handle. */
const moveTo = (x: number, y: number, init: PointerEventInit = {}) =>
  pointer('pointermove', outside, x, y, init);
const release = (x: number, y: number, init: PointerEventInit = {}) =>
  pointer('pointerup', outside, x, y, init);

function click(target: Element = handle): MouseEvent {
  const event = new MouseEvent('click', { bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

function keydown(key: string): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
  document.body.dispatchEvent(event);
  return event;
}

/** A press at 100,100 dragged to 110,100: past the threshold. */
function startDrag() {
  press(100, 100);
  moveTo(110, 100);
}

/** Run the frames asked for so far, at `time` ms. */
function runFrame(time: number) {
  const due = [...frames.values()];
  frames.clear();
  for (const callback of due) callback(time);
}

const dragMark = () => document.documentElement.getAttribute(POINTER_DRAG_ATTRIBUTE);

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  frames = new Map();
  nextFrameId = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    frames.set(++nextFrameId, callback);
    return nextFrameId;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => void frames.delete(id));

  calls = [];
  accepts = true;
  document.body.innerHTML = '';
  handle = document.createElement('button');
  label = document.createElement('span');
  handle.append(label);
  outside = document.createElement('div');
  document.body.append(handle, outside);
  clicks = vi.fn();
  handle.addEventListener('click', clicks);
});

afterEach(() => {
  action?.destroy?.();
  // A guard a test left up would swallow the next test's click.
  vi.runAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  document.documentElement.removeAttribute(POINTER_DRAG_ATTRIBUTE);
});

describe('a click', () => {
  it('still clicks when the press never moves', () => {
    attach();
    press(100, 100);
    release(100, 100);
    const event = click();

    expect(calls).toEqual([]);
    expect(clicks).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(false);
    expect(dragMark()).toBeNull();
  });

  it('still clicks when the press moves less than the threshold', () => {
    attach();
    press(100, 100);
    moveTo(102, 101);
    moveTo(103, 100);
    release(103, 100);
    click();

    expect(calls).toEqual([]);
    expect(clicks).toHaveBeenCalledTimes(1);
    expect(dragMark()).toBeNull();
  });

  it('measures the threshold from the press, not from the last move', () => {
    attach();
    press(100, 100);
    // Three 2 px moves in a circle never get 4 px from the press.
    moveTo(102, 100);
    moveTo(102, 102);
    moveTo(100, 102);
    release(100, 102);

    expect(calls).toEqual([]);
  });

  it('forgets the press once released', () => {
    attach();
    press(100, 100);
    release(100, 100);
    // The pointer moves on with no button down, or even with one.
    moveTo(200, 200, { buttons: 0 });
    moveTo(300, 300);

    expect(calls).toEqual([]);
  });
});

describe('a drag', () => {
  it('starts at the threshold, with the press point, then moves to where the pointer is', () => {
    attach();
    press(100, 100);
    expect(calls).toEqual([]);
    moveTo(104, 100);

    expect(calls).toEqual(['start 100,100', 'move 104,100']);
  });

  it('moves with the pointer, wherever in the window it goes', () => {
    attach();
    startDrag();
    moveTo(250, 40);
    moveTo(-30, 900);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'move 250,40', 'move -30,900']);
  });

  it('drops at the release point, and is over', () => {
    attach();
    startDrag();
    release(180, 120);
    moveTo(400, 400);
    release(400, 400);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'drop 180,120']);
  });

  it('starts from a press on something inside the handle', () => {
    attach();
    pointer('pointerdown', label, 100, 100);
    moveTo(110, 100);

    expect(calls).toEqual(['start 100,100', 'move 110,100']);
  });

  it('does not start from a press outside the handle', () => {
    attach();
    pointer('pointerdown', outside, 100, 100);
    moveTo(110, 100);

    expect(calls).toEqual([]);
  });

  it('can be made again after one ends', () => {
    attach();
    startDrag();
    release(110, 100);
    press(50, 50);
    moveTo(50, 60);
    release(50, 70);

    expect(calls.slice(3)).toEqual(['start 50,50', 'move 50,60', 'drop 50,70']);
  });

  it('ignores another pointer while one is down', () => {
    attach();
    startDrag();
    moveTo(500, 500, { pointerId: 2 });
    release(500, 500, { pointerId: 2 });
    pointer('pointercancel', outside, 500, 500, { pointerId: 2 });
    moveTo(120, 100);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'move 120,100']);
  });

  it('uses the callbacks of the latest options', () => {
    attach();
    startDrag();
    action.update?.(surface({ drop: (point) => calls.push(`later drop ${xy(point)}`) }));
    release(110, 100);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'later drop 110,100']);
  });
});

describe('what arms the handle', () => {
  it('a pen does', () => {
    attach();
    press(100, 100, { pointerType: 'pen' });
    moveTo(110, 100, { pointerType: 'pen' });

    expect(calls).toEqual(['start 100,100', 'move 110,100']);
  });

  it('a finger does not', () => {
    attach();
    press(100, 100, { pointerType: 'touch' });
    moveTo(110, 100, { pointerType: 'touch' });
    release(110, 100, { pointerType: 'touch' });

    expect(calls).toEqual([]);
    expect(dragMark()).toBeNull();
  });

  it('another button does not', () => {
    attach();
    press(100, 100, { button: 2, buttons: 2 });
    moveTo(110, 100, { buttons: 2 });
    press(100, 100, { button: 1, buttons: 4 });
    moveTo(110, 100, { buttons: 4 });
    // Nor once the primary button joins it: that sends a move, not a press.
    press(100, 100, { button: 2, buttons: 2 });
    moveTo(110, 100, { buttons: 3 });

    expect(calls).toEqual([]);
  });

  it('a pointer that is not the primary one does not', () => {
    attach();
    press(100, 100, { isPrimary: false });
    moveTo(110, 100, { isPrimary: false });

    expect(calls).toEqual([]);
  });

  it('nothing does while the handle is off', () => {
    attach({ enabled: false });
    startDrag();

    expect(calls).toEqual([]);

    // And on again.
    release(110, 100);
    action.update?.(surface({ enabled: true }));
    startDrag();
    expect(calls).toEqual(['start 100,100', 'move 110,100']);
  });
});

describe('the mark on the root', () => {
  it('is there from the start of the drag to its end', () => {
    attach();
    press(100, 100);
    moveTo(102, 100);
    expect(dragMark()).toBeNull();
    moveTo(110, 100);
    expect(dragMark()).toBe('active');
    release(110, 100);
    expect(dragMark()).toBeNull();
  });

  it('says refused where a drop would not act, and active again where it would', () => {
    attach();
    accepts = false;
    startDrag();
    expect(dragMark()).toBe('refused');
    accepts = true;
    moveTo(120, 100);
    expect(dragMark()).toBe('active');
    accepts = false;
    moveTo(130, 100);
    expect(dragMark()).toBe('refused');
  });

  it('leaves a refused release to the surface: it is still a drop', () => {
    attach();
    accepts = false;
    startDrag();
    release(110, 100);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'drop 110,100']);
    expect(dragMark()).toBeNull();
  });

  it('is not cleared by another handle that only clicks or unmounts', () => {
    attach();
    const other = document.createElement('button');
    document.body.append(other);
    const otherAction = pointerDrag(other, surface());
    startDrag();

    // A second pointer clicks the other handle, which then unmounts.
    pointer('pointerdown', other, 300, 300, { pointerId: 2 });
    pointer('pointerup', other, 300, 300, { pointerId: 2 });
    otherAction.destroy?.();

    expect(dragMark()).toBe('active');
  });

  it('is cleared before the surface hears of the end', () => {
    const marks: (string | null)[] = [];
    attach({ drop: () => marks.push(dragMark()), cancel: () => marks.push(dragMark()) });
    startDrag();
    release(110, 100);
    startDrag();
    keydown('Escape');

    expect(marks).toEqual([null, null]);
  });
});

describe('cancelling', () => {
  /** The drag is over: nothing more reaches the surface, and the mark is gone. */
  function expectCancelled() {
    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
    expect(dragMark()).toBeNull();
    moveTo(200, 200);
    release(200, 200);
    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
  }

  it('Escape cancels', () => {
    attach();
    startDrag();
    keydown('Escape');

    expectCancelled();
  });

  it('Escape is spent on the drag: nothing else hears it', () => {
    attach();
    const heard = vi.fn();
    document.addEventListener('keydown', heard, true);
    startDrag();
    const event = keydown('Escape');
    document.removeEventListener('keydown', heard, true);

    expect(heard).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
  });

  it('another key does not cancel, and is left alone', () => {
    attach();
    const heard = vi.fn();
    document.addEventListener('keydown', heard, true);
    startDrag();
    const event = keydown('a');
    document.removeEventListener('keydown', heard, true);

    expect(calls).toEqual(['start 100,100', 'move 110,100']);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(false);
  });

  it('Escape is left alone when no drag is under way, even with the button down', () => {
    attach();
    const heard = vi.fn();
    document.addEventListener('keydown', heard, true);
    const idle = keydown('Escape');
    press(100, 100);
    const armed = keydown('Escape');
    document.removeEventListener('keydown', heard, true);

    expect(heard).toHaveBeenCalledTimes(2);
    expect(idle.defaultPrevented).toBe(false);
    expect(armed.defaultPrevented).toBe(false);
    expect(calls).toEqual([]);
  });

  it('a pointercancel cancels', () => {
    attach();
    startDrag();
    pointer('pointercancel', outside, 110, 100);

    expectCancelled();
  });

  it('the handle losing the capture cancels', () => {
    handle.setPointerCapture = vi.fn();
    handle.releasePointerCapture = vi.fn();
    attach();
    startDrag();
    pointer('lostpointercapture', handle, 110, 100);

    expectCancelled();
  });

  it('something else losing a capture does not', () => {
    handle.setPointerCapture = vi.fn();
    handle.releasePointerCapture = vi.fn();
    attach();
    startDrag();
    // The element the press landed on gives its implicit capture up to the handle.
    pointer('lostpointercapture', label, 110, 100);
    pointer('lostpointercapture', handle, 110, 100, { pointerId: 2 });

    expect(calls).toEqual(['start 100,100', 'move 110,100']);
  });

  it('the capture given up by the release does not cancel a drop', () => {
    handle.setPointerCapture = vi.fn();
    handle.releasePointerCapture = vi.fn();
    attach();
    startDrag();
    release(110, 100);
    pointer('lostpointercapture', handle, 110, 100);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'drop 110,100']);
  });

  it('the window losing focus cancels', () => {
    attach();
    startDrag();
    window.dispatchEvent(new FocusEvent('blur'));

    expectCancelled();
  });

  it('an element losing focus does not', () => {
    attach();
    const field = document.createElement('input');
    document.body.append(field);
    startDrag();
    field.dispatchEvent(new FocusEvent('blur'));
    field.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));

    expect(calls).toEqual(['start 100,100', 'move 110,100']);
  });

  it('the handle unmounting cancels', () => {
    attach();
    startDrag();
    action.destroy?.();

    expectCancelled();
  });

  it('the handle being turned off cancels', () => {
    attach();
    startDrag();
    action.update?.(surface({ enabled: false }));

    expectCancelled();
  });

  it('a release the page never saw cancels on the next move', () => {
    attach();
    startDrag();
    // Back over the window with no button down.
    moveTo(150, 150, { buttons: 0 });

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
    expect(dragMark()).toBeNull();
  });

  it('cancels once, however many ways the drag ends', () => {
    attach();
    startDrag();
    keydown('Escape');
    window.dispatchEvent(new FocusEvent('blur'));
    pointer('pointercancel', outside, 110, 100);
    action.update?.(surface({ enabled: false }));
    action.destroy?.();

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
  });

  it('tells the surface nothing of a press that ends before it is a drag', () => {
    const endings: (() => void)[] = [
      () => pointer('pointercancel', outside, 100, 100),
      () => window.dispatchEvent(new FocusEvent('blur')),
      () => moveTo(101, 100, { buttons: 0 }),
      () => action.update?.(surface({ enabled: false })),
      () => action.destroy?.(),
    ];
    for (const end of endings) {
      attach();
      press(100, 100);
      end();
      action.destroy?.();
    }

    expect(calls).toEqual([]);
  });

  it('forgets a press that was cancelled before it was a drag', () => {
    attach();
    press(100, 100);
    pointer('pointercancel', outside, 100, 100);
    moveTo(110, 100);

    expect(calls).toEqual([]);
  });

  it('a new press cancels a drag whose release never came', () => {
    attach();
    startDrag();
    press(300, 300);
    moveTo(310, 300);

    expect(calls).toEqual([
      'start 100,100',
      'move 110,100',
      'cancel',
      'start 300,300',
      'move 310,300',
    ]);
  });

  it('leaves nothing behind when a callback throws', () => {
    attach({
      cancel: () => {
        throw new Error('surface failed');
      },
    });
    startDrag();
    // jsdom reports a listener's error instead of throwing it from dispatch.
    const reported = vi.fn((event: ErrorEvent) => event.preventDefault());
    window.addEventListener('error', reported);
    keydown('Escape');
    window.removeEventListener('error', reported);

    expect(reported).toHaveBeenCalledTimes(1);
    expect(dragMark()).toBeNull();
    moveTo(200, 200);
    expect(calls).toEqual(['start 100,100', 'move 110,100']);
  });
});

describe('the click after a drag', () => {
  it('is swallowed', () => {
    attach();
    startDrag();
    release(110, 100);
    const event = click();

    expect(clicks).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
  });

  it('is swallowed once: the next click is its own', () => {
    attach();
    startDrag();
    release(110, 100);
    click();
    const next = click();

    expect(clicks).toHaveBeenCalledTimes(1);
    expect(next.defaultPrevented).toBe(false);
  });

  it('is not waited for: a release that sends none does not cost a later click', () => {
    attach();
    startDrag();
    release(110, 100);
    // The click would have come in the same task as the release.
    vi.runAllTimers();
    const later = click();

    expect(clicks).toHaveBeenCalledTimes(1);
    expect(later.defaultPrevented).toBe(false);
  });

  it('is not taken from a new press, even one made at once', () => {
    attach();
    startDrag();
    release(110, 100);
    press(110, 100);
    release(110, 100);
    click();

    expect(clicks).toHaveBeenCalledTimes(1);
  });

  it('is swallowed wherever it lands', () => {
    attach();
    const heard = vi.fn();
    outside.addEventListener('click', heard);
    startDrag();
    release(110, 100);
    click(outside);

    expect(heard).not.toHaveBeenCalled();
  });

  it('is swallowed after a drag that Escape cancelled, when the button comes up', () => {
    attach();
    startDrag();
    keydown('Escape');
    // The button is still down: no guard yet, and none needed.
    release(110, 100);
    click();

    expect(clicks).not.toHaveBeenCalled();
    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
  });

  it('is swallowed after a drag cancelled by the window losing focus or the handle turning off', () => {
    attach();
    startDrag();
    window.dispatchEvent(new FocusEvent('blur'));
    release(110, 100);
    click();
    vi.runAllTimers();

    startDrag();
    action.update?.(surface({ enabled: false }));
    release(110, 100);
    click();

    expect(clicks).not.toHaveBeenCalled();
  });

  it('is not swallowed after a press that never became a drag and was turned off', () => {
    attach();
    press(100, 100);
    action.update?.(surface({ enabled: false }));
    release(100, 100);
    click();

    expect(clicks).toHaveBeenCalledTimes(1);
  });

  it('leaves no guard up once the handle unmounts', () => {
    attach();
    startDrag();
    release(110, 100);
    action.destroy?.();
    click();

    expect(clicks).toHaveBeenCalledTimes(1);
  });
});

describe('pointer capture', () => {
  it('is taken when the drag starts, not on the press, and given back at the end', () => {
    const capture = vi.fn();
    const give = vi.fn();
    handle.setPointerCapture = capture;
    handle.releasePointerCapture = give;
    attach();

    press(100, 100, { pointerId: 7 });
    moveTo(102, 100, { pointerId: 7 });
    // A captured pointer's click goes to the handle, not to a button in it.
    expect(capture).not.toHaveBeenCalled();
    moveTo(110, 100, { pointerId: 7 });
    expect(capture).toHaveBeenCalledExactlyOnceWith(7);
    expect(give).not.toHaveBeenCalled();
    release(110, 100, { pointerId: 7 });
    expect(give).toHaveBeenCalledExactlyOnceWith(7);
  });

  it('is given back when the drag is cancelled', () => {
    const give = vi.fn();
    handle.setPointerCapture = vi.fn();
    handle.releasePointerCapture = give;
    attach();
    startDrag();
    keydown('Escape');

    expect(give).toHaveBeenCalledExactlyOnceWith(1);
  });

  it('is not needed: a drag runs where the engine has none, or refuses it', () => {
    // jsdom has no pointer capture at all.
    expect(handle.setPointerCapture).toBeUndefined();
    attach();
    startDrag();
    release(120, 100);
    expect(calls).toEqual(['start 100,100', 'move 110,100', 'drop 120,100']);

    calls = [];
    handle.setPointerCapture = () => {
      throw new DOMException('no such pointer', 'NotFoundError');
    };
    handle.releasePointerCapture = vi.fn();
    startDrag();
    release(120, 100);
    expect(calls).toEqual(['start 100,100', 'move 110,100', 'drop 120,100']);
    expect(handle.releasePointerCapture).not.toHaveBeenCalled();
  });
});

describe('a native drag from the handle', () => {
  function nativeDrag(target: Element): Event {
    const event = new Event('dragstart', { bubbles: true, cancelable: true });
    target.dispatchEvent(event);
    return event;
  }

  it('is refused, so it cannot take the pointer', () => {
    attach();
    expect(nativeDrag(label).defaultPrevented).toBe(true);
    expect(nativeDrag(handle).defaultPrevented).toBe(true);
  });

  it('is left alone outside the handle, while it is off, and once it unmounts', () => {
    attach();
    expect(nativeDrag(outside).defaultPrevented).toBe(false);
    action.update?.(surface({ enabled: false }));
    expect(nativeDrag(label).defaultPrevented).toBe(false);
    action.update?.(surface());
    action.destroy?.();
    expect(nativeDrag(label).defaultPrevented).toBe(false);
  });
});

describe('scrolling near an edge', () => {
  let scroller: HTMLDivElement;

  /** A scroller from 100 to 500 across and 0 to 300 down: 48 px zones at each edge. */
  beforeEach(() => {
    scroller = document.createElement('div');
    document.body.append(scroller);
    scroller.getBoundingClientRect = () => new DOMRect(100, 0, 400, 300);
  });

  const across = (overrides: Partial<PointerDragOptions> = {}) =>
    attach({ scroll: { container: () => scroller, axis: 'x' }, ...overrides });

  /** A press in the scroller's middle, dragged to `x`. */
  function dragAcrossTo(x: number, y = 150) {
    press(300, 150);
    moveTo(x, y);
    calls = [];
  }

  it('scrolls on while the pointer is at the end edge, re-issuing the move each frame', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    // The first frame only starts the clock.
    expect(scroller.scrollLeft).toBe(0);
    runFrame(1020);
    // Full speed, 900 px a second, over 20 ms.
    expect(scroller.scrollLeft).toBe(18);
    runFrame(1040);
    expect(scroller.scrollLeft).toBe(36);

    // The pointer never moved: each scroll put something new under it.
    expect(calls).toEqual(['move 500,150', 'move 500,150']);
  });

  it('scrolls back at the start edge', () => {
    across();
    scroller.scrollLeft = 200;
    dragAcrossTo(100);
    runFrame(1000);
    runFrame(1020);

    expect(scroller.scrollLeft).toBe(182);
    expect(calls).toEqual(['move 100,150']);
  });

  it('scrolls slower further from the edge', () => {
    across();
    // Halfway into the end zone: half speed, 9 px over 20 ms.
    dragAcrossTo(476);
    runFrame(1000);
    runFrame(1020);

    expect(scroller.scrollLeft).toBe(9);
  });

  it('scrolls nothing in the middle, and re-issues no move', () => {
    across();
    dragAcrossTo(350);
    runFrame(1000);
    runFrame(1020);
    runFrame(1040);

    expect(scroller.scrollLeft).toBe(0);
    expect(calls).toEqual([]);
  });

  it('follows the pointer in and out of a zone', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);
    moveTo(350, 150);
    runFrame(1040);
    runFrame(1060);
    expect(scroller.scrollLeft).toBe(18);
    moveTo(500, 150);
    runFrame(1080);
    expect(scroller.scrollLeft).toBe(36);
  });

  it('shows what the re-issued move answers', () => {
    across();
    dragAcrossTo(500);
    expect(dragMark()).toBe('active');
    // The scroll brings a place that refuses the drop under the pointer.
    accepts = false;
    runFrame(1000);
    runFrame(1020);

    expect(dragMark()).toBe('refused');
  });

  it('re-issues no move when the container is at its end and does not scroll', () => {
    across();
    Object.defineProperty(scroller, 'scrollLeft', { get: () => 640, set: () => {} });
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);
    runFrame(1040);

    expect(calls).toEqual([]);
  });

  it('scrolls down a container that scrolls on the y axis', () => {
    attach({ scroll: { container: () => scroller, axis: 'y' } });
    press(300, 150);
    moveTo(300, 300);
    calls = [];
    runFrame(1000);
    runFrame(1020);

    expect(scroller.scrollTop).toBe(18);
    expect(scroller.scrollLeft).toBe(0);
    expect(calls).toEqual(['move 300,300']);
  });

  it('scrolls nothing while the pointer is beside the container, not over it', () => {
    across();
    // Level with the end edge, but below the scroller.
    dragAcrossTo(500, 320);
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(0);

    // And above it, on the y axis's side.
    moveTo(500, -1);
    runFrame(1040);
    expect(scroller.scrollLeft).toBe(0);

    moveTo(500, 299);
    runFrame(1060);
    expect(scroller.scrollLeft).toBe(18);
  });

  it('keeps the fraction of a pixel a slow frame leaves', () => {
    across();
    // 2 px into the end zone: 37.5 px a second, 0.6 px a 16 ms frame.
    dragAcrossTo(454);
    const positions: number[] = [];
    for (let frame = 0; frame <= 4; frame++) {
      runFrame(1000 + frame * 16);
      positions.push(scroller.scrollLeft);
    }

    // 0, 0.6, 1.2, 1.8 and 2.4 px in all: a frame alone never adds a whole one.
    expect(positions).toEqual([0, 0, 1, 1, 2]);
  });

  it('does not jump after a stalled frame', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(9000);

    // A tenth of a second's worth, not eight seconds'.
    expect(scroller.scrollLeft).toBe(90);
  });

  it('does not scroll at once for a press made inside an edge zone', () => {
    across();
    scroller.scrollLeft = 500;
    // Pressed 20 px from the start edge, then dragged away from it, still in the zone.
    press(120, 150);
    moveTo(130, 150);
    runFrame(1000);
    runFrame(1020);
    runFrame(1040);

    expect(scroller.scrollLeft).toBe(500);
  });

  it('scrolls for such a press once the pointer moves nearer the edge', () => {
    across();
    scroller.scrollLeft = 500;
    press(120, 150);
    moveTo(112, 150);
    runFrame(1000);
    runFrame(1020);

    // 36 px into the zone: three quarters of full speed, 13.5 px, the half carried.
    expect(scroller.scrollLeft).toBe(487);
  });

  it('scrolls for such a press once the pointer has left the zone and come back', () => {
    across();
    scroller.scrollLeft = 500;
    press(120, 150);
    moveTo(200, 150);
    runFrame(1000);
    moveTo(130, 150);
    runFrame(1020);

    // 18 px into the zone, further from the edge than the press: 6.75 px.
    expect(scroller.scrollLeft).toBe(494);
  });

  it('holds back the end edge the same way', () => {
    across();
    press(480, 150);
    moveTo(470, 150);
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(0);

    moveTo(490, 150);
    runFrame(1040);
    // 38 px into the zone: 14.25 px.
    expect(scroller.scrollLeft).toBe(14);
  });

  it('lets the end edge go once the pointer has left its zone and come back', () => {
    across();
    press(480, 150);
    moveTo(300, 150);
    runFrame(1000);
    moveTo(470, 150);
    runFrame(1020);

    // 18 px into the zone, further from the edge than the press: 6.75 px.
    expect(scroller.scrollLeft).toBe(6);
  });

  it('holds an edge back afresh for each drag', () => {
    across();
    scroller.scrollLeft = 500;
    // One drag leaves the start zone and comes back, which lets the edge go.
    press(120, 150);
    moveTo(200, 150);
    runFrame(1000);
    moveTo(130, 150);
    runFrame(1020);
    release(130, 150);
    expect(scroller.scrollLeft).toBe(494);

    // The next, pressed in the zone again, is held back again.
    press(120, 150);
    moveTo(130, 150);
    runFrame(2000);
    runFrame(2020);
    expect(scroller.scrollLeft).toBe(494);
  });

  it('drops the carried fraction when the pointer leaves the zone', () => {
    across();
    // 0.6 px a 16 ms frame, as above: two frames in the zone carry 1.2 px.
    dragAcrossTo(454);
    runFrame(1000);
    runFrame(1016);
    moveTo(300, 150);
    runFrame(1032);
    moveTo(454, 150);
    runFrame(1048);

    // The 0.6 px carried before the pointer left did not wait for it.
    expect(scroller.scrollLeft).toBe(0);
  });

  it('stops when the drag drops', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    release(500, 150);

    expect(frames.size).toBe(0);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(0);
  });

  it('stops when the drag is cancelled', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    keydown('Escape');

    expect(frames.size).toBe(0);
  });

  it('stops when the re-issued move ends the drag', () => {
    across({
      move: (point) => {
        calls.push(`move ${xy(point)}`);
        // The surface turns the handle off from inside the move.
        if (scroller.scrollLeft > 0) action.update?.(surface({ enabled: false }));
        return true;
      },
    });
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);

    expect(calls).toEqual(['move 500,150', 'cancel']);
    expect(frames.size).toBe(0);
    expect(dragMark()).toBeNull();
  });

  it('asks for no frames before a drag, or with nothing to scroll', () => {
    across();
    press(300, 150);
    moveTo(302, 150);
    expect(frames.size).toBe(0);
    release(302, 150);
    action.destroy?.();

    attach();
    startDrag();
    expect(frames.size).toBe(0);
  });

  it('scrolls nothing while the container is not there', () => {
    let container: HTMLElement | null = null;
    attach({ scroll: { container: () => container, axis: 'x' } });
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(0);

    container = scroller;
    runFrame(1040);
    expect(scroller.scrollLeft).toBe(18);
  });

  it('takes a container given part-way through a drag, and stops when it is taken away', () => {
    attach();
    dragAcrossTo(500);
    expect(frames.size).toBe(0);

    action.update?.(surface({ scroll: { container: () => scroller, axis: 'x' } }));
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(18);

    action.update?.(surface());
    runFrame(1040);
    expect(frames.size).toBe(0);
    expect(scroller.scrollLeft).toBe(18);

    // And takes it back again.
    action.update?.(surface({ scroll: { container: () => scroller, axis: 'x' } }));
    runFrame(1060);
    runFrame(1080);
    expect(scroller.scrollLeft).toBe(36);
  });
});
