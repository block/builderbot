import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DRAG_CHIP_CLASS,
  DRAG_CHIP_MARGIN,
  DRAG_CHIP_OFFSET,
  DRAG_REACH,
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

  it('a press made elsewhere cancels a drag whose release never came, and its release drops nothing', () => {
    attach();
    startDrag();
    // The same pointer, pressed again: its button had come up unseen.
    pointer('pointerdown', outside, 300, 300);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
    expect(dragMark()).toBeNull();
    moveTo(310, 300);
    release(310, 300);
    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
  });

  it('a press made elsewhere ends a press whose release never came: it does not drag the handle', () => {
    attach();
    press(100, 100);
    pointer('pointerdown', outside, 300, 300);
    moveTo(310, 300);
    release(310, 300);

    expect(calls).toEqual([]);
  });

  it('another pointer pressed elsewhere does not end the drag', () => {
    attach();
    startDrag();
    pointer('pointerdown', outside, 300, 300, { pointerId: 2 });
    release(120, 100);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'drop 120,100']);
  });

  it('hears a press made elsewhere that stops on its way down', () => {
    attach();
    outside.addEventListener('pointerdown', (event) => event.stopPropagation());
    startDrag();
    pointer('pointerdown', outside, 300, 300);

    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
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

  it('is not taken from a press made elsewhere, after a cancelled drag whose release never came', () => {
    attach();
    const heard = vi.fn();
    outside.addEventListener('click', heard);
    startDrag();
    // The window lost focus mid-drag, and the release went to another one.
    window.dispatchEvent(new FocusEvent('blur'));
    // The click that brings the focus back reaches the page, with no move
    // before it: a whole new press, release and click, somewhere else.
    pointer('pointerdown', outside, 300, 300);
    release(300, 300);
    const later = click(outside);

    expect(heard).toHaveBeenCalledTimes(1);
    expect(later.defaultPrevented).toBe(false);
    expect(calls).toEqual(['start 100,100', 'move 110,100', 'cancel']);
  });

  it('is not taken from a press made elsewhere, after a drag whose release never came', () => {
    attach();
    const heard = vi.fn();
    outside.addEventListener('click', heard);
    startDrag();
    pointer('pointerdown', outside, 300, 300);
    release(300, 300);
    click(outside);

    expect(heard).toHaveBeenCalledTimes(1);
  });

  it('is still swallowed when another pointer is pressed before the button comes up', () => {
    attach();
    startDrag();
    keydown('Escape');
    pointer('pointerdown', outside, 300, 300, { pointerId: 2 });
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

describe('the chip that follows the pointer', () => {
  const chips = () => [...document.querySelectorAll<HTMLElement>(`.${DRAG_CHIP_CLASS}`)];
  const chip = () => chips()[0];

  /** An icon inside the handle, as a column's header has. */
  function icon(): SVGSVGElement {
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('style', 'color: red');
    svg.append(document.createElementNS('http://www.w3.org/2000/svg', 'circle'));
    handle.prepend(svg);
    return svg;
  }

  const named = (overrides: Partial<PointerDragOptions> = {}) =>
    attach({ image: () => ({ label: 'In progress' }), ...overrides });

  it('is not there for a press, and is once the press is a drag', () => {
    named();
    press(100, 100);
    expect(chips()).toEqual([]);
    moveTo(102, 100);
    expect(chips()).toEqual([]);
    moveTo(110, 100);

    expect(chips()).toHaveLength(1);
    expect(chip().textContent).toBe('In progress');
  });

  it('is never there for a click', () => {
    named();
    press(100, 100);
    release(100, 100);
    click();
    expect(chips()).toEqual([]);
    expect(clicks).toHaveBeenCalledTimes(1);
  });

  it('is a child of the body, out of the page’s flow, deaf to the pointer and to readers', () => {
    named();
    startDrag();

    expect(chip().parentElement).toBe(document.body);
    expect(chip().style.position).toBe('fixed');
    expect(chip().style.pointerEvents).toBe('none');
    expect(chip().getAttribute('aria-hidden')).toBe('true');
    // Nothing in it can take focus or a click either.
    expect(chip().querySelector('button, input, a, [tabindex]')).toBeNull();
  });

  it('hangs below the pointer and to its left, and follows it', () => {
    named();
    startDrag();
    // jsdom lays nothing out, so the chip has no size: its corner is its right edge.
    expect(chip().style.transform).toBe(
      `translate3d(${110 - DRAG_CHIP_OFFSET.x}px, ${100 + DRAG_CHIP_OFFSET.y}px, 0)`
    );

    moveTo(400, 250);
    expect(chip().style.transform).toBe(
      `translate3d(${400 - DRAG_CHIP_OFFSET.x}px, ${250 + DRAG_CHIP_OFFSET.y}px, 0)`
    );
    expect(chips()).toHaveLength(1);
  });

  it('is placed by its size, read once it is in the page, and kept in the window', () => {
    const width = vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(120);
    const height = vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(28);
    named();
    startDrag();
    moveTo(400, 250);
    expect(chip().style.transform).toBe(
      `translate3d(${400 - DRAG_CHIP_OFFSET.x - 120}px, ${250 + DRAG_CHIP_OFFSET.y}px, 0)`
    );

    // At the window's left edge it slides right, not off the window.
    moveTo(30, 250);
    expect(chip().style.transform).toBe(
      `translate3d(${DRAG_CHIP_MARGIN}px, ${250 + DRAG_CHIP_OFFSET.y}px, 0)`
    );
    // And up at its bottom edge.
    moveTo(400, window.innerHeight - 2);
    expect(chip().style.transform).toBe(
      `translate3d(${400 - DRAG_CHIP_OFFSET.x - 120}px, ${window.innerHeight - DRAG_CHIP_MARGIN - 28}px, 0)`
    );
    width.mockRestore();
    height.mockRestore();
  });

  it('leads the name with a copy of the icon, and leaves the handle its own', () => {
    const svg = icon();
    attach({ image: (node) => ({ label: 'Done', icon: node.querySelector('svg') }) });
    startDrag();

    const copy = chip().querySelector('svg')!;
    expect(copy).not.toBe(svg);
    expect(copy.getAttribute('style')).toBe('color: red');
    expect(copy.querySelector('circle')).not.toBeNull();
    expect(chip().firstElementChild).toBe(copy);
    expect(chip().textContent).toBe('Done');
    expect(handle.contains(svg)).toBe(true);
  });

  it('is given the handle to read the image from', () => {
    const image = vi.fn(() => ({ label: 'Done' }));
    attach({ image });
    startDrag();
    expect(image).toHaveBeenCalledExactlyOnceWith(handle);
  });

  it('shows a name as text, not as markup', () => {
    attach({ image: () => ({ label: '<b>bold</b>' }) });
    startDrag();
    expect(chip().textContent).toBe('<b>bold</b>');
    expect(chip().querySelector('b')).toBeNull();
  });

  it('is asked for once, as the drag starts, after the surface heard of it', () => {
    const image = vi.fn(() => {
      calls.push('image');
      return { label: 'Done' };
    });
    attach({ image });
    press(100, 100);
    expect(image).not.toHaveBeenCalled();
    moveTo(110, 100);
    moveTo(120, 100);
    release(120, 100);

    expect(calls).toEqual([
      'start 100,100',
      'image',
      'move 110,100',
      'move 120,100',
      'drop 120,100',
    ]);
  });

  it('is not there without an image, or with none to show', () => {
    attach();
    startDrag();
    expect(chips()).toEqual([]);
    release(110, 100);
    action.destroy?.();

    attach({ image: () => null });
    startDrag();
    expect(chips()).toEqual([]);
    expect(calls).toContain('move 110,100');
  });

  it('goes on the drop, before the surface hears of it', () => {
    let chipsAtDrop = -1;
    named({ drop: () => (chipsAtDrop = chips().length) });
    startDrag();
    release(300, 100);

    expect(chipsAtDrop).toBe(0);
    expect(chips()).toEqual([]);
  });

  it('goes on a drop the surface refuses', () => {
    accepts = false;
    named();
    startDrag();
    expect(chips()).toHaveLength(1);
    release(110, 100);
    expect(chips()).toEqual([]);
  });

  it.each([
    ['Escape', () => void keydown('Escape')],
    ['a pointercancel', () => void pointer('pointercancel', outside, 110, 100)],
    [
      'the handle losing the capture',
      () => void pointer('lostpointercapture', handle, 110, 100, { bubbles: true }),
    ],
    ['the window losing focus', () => void window.dispatchEvent(new FocusEvent('blur'))],
    ['a release the page never saw', () => void moveTo(120, 100, { buttons: 0 })],
    [
      'the handle being turned off',
      (cancel: () => void) => action.update?.(surface({ enabled: false, cancel })),
    ],
    ['the handle unmounting', () => action.destroy?.()],
    ['a new press', () => void press(100, 100)],
  ])('goes when %s cancels the drag, before the surface hears of it', (_way, end) => {
    let chipsAtCancel = -1;
    const cancel = () => void (chipsAtCancel = chips().length);
    named({ cancel });
    startDrag();
    expect(chips()).toHaveLength(1);

    end(cancel);

    expect(chipsAtCancel).toBe(0);
    expect(chips()).toEqual([]);
    // Nothing brings it back: not a move, nor the release still to come.
    moveTo(130, 100);
    release(130, 100);
    expect(chips()).toEqual([]);
  });

  it('goes when a callback throws on the way out', () => {
    named({
      drop: () => {
        throw new Error('drop failed');
      },
    });
    startDrag();
    // jsdom reports a listener's error instead of throwing it from dispatch.
    const reported = vi.fn((event: ErrorEvent) => event.preventDefault());
    window.addEventListener('error', reported);
    release(110, 100);
    window.removeEventListener('error', reported);

    expect(reported).toHaveBeenCalledTimes(1);
    expect(chips()).toEqual([]);
  });

  it('goes when the first move ends the drag by turning the handle off', () => {
    named({
      move: () => {
        action.update?.(surface({ enabled: false }));
        return true;
      },
    });
    startDrag();
    expect(chips()).toEqual([]);
  });

  it('is not put up when the start ends the drag', () => {
    const image = vi.fn(() => ({ label: 'Done' }));
    attach({
      image,
      start: () => action.update?.(surface({ enabled: false })),
    });
    startDrag();
    expect(image).not.toHaveBeenCalled();
    expect(chips()).toEqual([]);
  });

  it('is a new one for each drag, with that drag’s image', () => {
    let name = 'Idea';
    attach({ image: () => ({ label: name }) });
    startDrag();
    release(110, 100);
    name = 'Done';
    startDrag();

    expect(chips()).toHaveLength(1);
    expect(chip().textContent).toBe('Done');
  });

  it('stays where the pointer is while the container scrolls under it', () => {
    const scroller = document.createElement('div');
    document.body.append(scroller);
    scroller.getBoundingClientRect = () => new DOMRect(100, 0, 400, 300);
    named({ scroll: { container: () => scroller, axis: 'x' } });
    press(300, 150);
    moveTo(500, 150);
    const placed = chip().style.transform;
    runFrame(1000);
    runFrame(1020);

    expect(scroller.scrollLeft).toBe(18);
    expect(chip().style.transform).toBe(placed);
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

  it('scrolls on for a pointer that strays off the container’s side, within reach', () => {
    across();
    // Level with the end edge, and as far below the scroller as still counts.
    dragAcrossTo(500, 299 + DRAG_REACH);
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(18);

    // And as far above it.
    moveTo(500, -DRAG_REACH);
    runFrame(1040);
    expect(scroller.scrollLeft).toBe(36);
  });

  it('scrolls nothing once the pointer is out of reach of the container’s side', () => {
    across();
    dragAcrossTo(500, 300 + DRAG_REACH);
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(0);

    moveTo(500, -1 - DRAG_REACH);
    runFrame(1040);
    expect(scroller.scrollLeft).toBe(0);

    // Back within reach, it scrolls again.
    moveTo(500, 299 + DRAG_REACH);
    runFrame(1060);
    expect(scroller.scrollLeft).toBe(18);
  });

  it('scrolls at full speed for a pointer past an end, within reach, and not beyond it', () => {
    across();
    dragAcrossTo(499 + DRAG_REACH);
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(18);

    // Out of reach past the end: the surface has given the drag up there.
    moveTo(500 + DRAG_REACH, 150);
    runFrame(1040);
    runFrame(1060);
    expect(scroller.scrollLeft).toBe(18);

    // The start edge the same.
    moveTo(100 - DRAG_REACH, 150);
    runFrame(1080);
    expect(scroller.scrollLeft).toBe(0);
    scroller.scrollLeft = 100;
    moveTo(99 - DRAG_REACH, 150);
    runFrame(1100);
    expect(scroller.scrollLeft).toBe(100);
  });

  it('gives a list the same reach, off its sides and past its ends', () => {
    attach({ scroll: { container: () => scroller, axis: 'y' } });
    press(300, 150);
    // Off the list's left side, level with its bottom edge.
    moveTo(100 - DRAG_REACH, 300);
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollTop).toBe(18);

    moveTo(99 - DRAG_REACH, 300);
    runFrame(1040);
    expect(scroller.scrollTop).toBe(18);

    // Past its bottom end.
    moveTo(300, 299 + DRAG_REACH);
    runFrame(1060);
    expect(scroller.scrollTop).toBe(36);
    moveTo(300, 300 + DRAG_REACH);
    runFrame(1080);
    expect(scroller.scrollTop).toBe(36);
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

describe('a scroll under a held drag', () => {
  let scroller: HTMLDivElement;

  /** A scroller from 100 to 500 across and 0 to 300 down, as above. */
  beforeEach(() => {
    scroller = document.createElement('div');
    document.body.append(scroller);
    scroller.getBoundingClientRect = () => new DOMRect(100, 0, 400, 300);
  });

  const across = (overrides: Partial<PointerDragOptions> = {}) =>
    attach({ scroll: { container: () => scroller, axis: 'x' }, ...overrides });

  /** A press in the scroller's middle, dragged to `x`. */
  function dragAcrossTo(x: number) {
    press(300, 150);
    moveTo(x, 150);
    calls = [];
  }

  /** A scroll event as the engines send one from an element: it does not bubble. */
  const scrolled = (target: EventTarget) => target.dispatchEvent(new Event('scroll'));

  it('of the container, by the wheel, re-issues the move where the pointer is', () => {
    across();
    // In the middle, where the drag itself scrolls nothing.
    dragAcrossTo(310);
    scroller.scrollLeft = 300;
    scrolled(scroller);

    expect(calls).toEqual(['move 310,150']);
  });

  it('shows what the re-issued move answers', () => {
    across();
    dragAcrossTo(310);
    accepts = false;
    scrolled(scroller);
    expect(dragMark()).toBe('refused');

    accepts = true;
    scrolled(scroller);
    expect(dragMark()).toBe('active');
  });

  it('of anything the handle is inside re-issues the move, with no container to scroll', () => {
    const list = document.createElement('div');
    const section = document.createElement('div');
    document.body.append(list);
    list.append(section);
    section.append(handle);
    attach();
    startDrag();
    calls = [];

    scrolled(section);
    scrolled(list);
    // The page itself: its scroll event comes from the document.
    scrolled(document);

    expect(calls).toEqual(['move 110,100', 'move 110,100', 'move 110,100']);
  });

  it('of something the handle is not inside re-issues nothing', () => {
    across();
    dragAcrossTo(310);
    const cards = document.createElement('div');
    scroller.append(cards);
    scrolled(cards);
    scrolled(outside);

    expect(calls).toEqual([]);
  });

  it('re-issues nothing before the press is a drag, or once the drag is over', () => {
    across();
    press(300, 150);
    scrolled(scroller);
    moveTo(302, 150);
    scrolled(scroller);
    expect(calls).toEqual([]);

    moveTo(310, 150);
    release(310, 150);
    calls = [];
    scrolled(scroller);
    expect(calls).toEqual([]);

    // Cancelled, with the button still down.
    press(300, 150);
    moveTo(310, 150);
    keydown('Escape');
    calls = [];
    scrolled(scroller);
    expect(calls).toEqual([]);
  });

  it('re-issues nothing for the event the drag’s own scroll step sends', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);
    expect(scroller.scrollLeft).toBe(18);
    expect(calls).toEqual(['move 500,150']);

    // The engine reports the step a frame later. The step re-issued the move
    // as it scrolled.
    scrolled(scroller);
    expect(calls).toEqual(['move 500,150']);

    runFrame(1040);
    scrolled(scroller);
    expect(scroller.scrollLeft).toBe(36);
    expect(calls).toEqual(['move 500,150', 'move 500,150']);
  });

  it('re-issues the move when the wheel scrolled on top of the drag’s own step', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);
    // One event reports both: the step to 18, and the wheel on to 200.
    scroller.scrollLeft = 200;
    scrolled(scroller);

    expect(calls).toEqual(['move 500,150', 'move 500,150']);
  });

  it('does not take another scroller’s event for the step’s own', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);
    // The page scrolled too, and its event comes first.
    scrolled(document);
    expect(calls).toEqual(['move 500,150', 'move 500,150']);

    // The step's own event is still to come, and is still not news.
    scrolled(scroller);
    expect(calls).toEqual(['move 500,150', 'move 500,150']);
  });

  it('lets a step answer for one event only: the wheel coming back to that offset is news', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);
    scrolled(scroller);
    // Out of the edge zone: the drag itself scrolls no more.
    moveTo(300, 150);
    runFrame(1040);
    calls = [];

    scroller.scrollLeft = 200;
    scrolled(scroller);
    scroller.scrollLeft = 18;
    scrolled(scroller);

    expect(calls).toEqual(['move 300,150', 'move 300,150']);
  });

  it('forgets a step the drag ended on: the next drag’s first scroll is news', () => {
    across();
    dragAcrossTo(500);
    runFrame(1000);
    runFrame(1020);
    release(500, 150);

    // The container is still where the step left it.
    dragAcrossTo(310);
    scrolled(scroller);

    expect(calls).toEqual(['move 310,150']);
  });

  it('follows a container given part-way through a drag', () => {
    attach();
    startDrag();
    calls = [];
    scrolled(scroller);
    expect(calls).toEqual([]);

    action.update?.(surface({ scroll: { container: () => scroller, axis: 'x' } }));
    scrolled(scroller);
    expect(calls).toEqual(['move 110,100']);
  });

  it('ends the drag cleanly when the re-issued move turns the handle off', () => {
    across({
      move: (point) => {
        calls.push(`move ${xy(point)}`);
        if (scroller.scrollLeft === 300) {
          action.update?.(surface({ enabled: false }));
        }
        return true;
      },
    });
    dragAcrossTo(310);
    scroller.scrollLeft = 300;
    scrolled(scroller);

    expect(calls).toEqual(['move 310,150', 'cancel']);
    expect(dragMark()).toBeNull();
  });
});
