import { describe, expect, it, vi } from 'vitest';
import { createTouchSelectionGuard, TAP_SLOP_PX } from './touchSelectionGuard';

function touch(x: number, y: number, pointerId?: number) {
  return { pointerType: 'touch', clientX: x, clientY: y, pointerId };
}

function mouse(x: number, y: number) {
  return { pointerType: 'mouse', clientX: x, clientY: y };
}

/** A pointer event as the window would dispatch it. */
function pointerEvent(
  type: string,
  pointerType: string,
  clientX: number,
  clientY: number,
  pointerId = 1
): Event {
  return Object.assign(new Event(type), { pointerType, clientX, clientY, pointerId });
}

describe('createTouchSelectionGuard', () => {
  it('allows a mousedown with no pointer events before it', () => {
    expect(createTouchSelectionGuard(TAP_SLOP_PX, null).allowsMouseDown()).toBe(true);
  });

  it('allows a mouse press while it is down', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown(mouse(10, 10));
    expect(guard.allowsMouseDown()).toBe(true);
    guard.pointerEnd(mouse(10, 10));
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('allows the compatibility mousedown of a touch that stayed put, once', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown(touch(100, 200));
    guard.pointerMove(touch(100 + TAP_SLOP_PX, 200 - TAP_SLOP_PX));
    guard.pointerEnd(touch(100 + TAP_SLOP_PX, 200 - TAP_SLOP_PX));
    expect(guard.allowsMouseDown()).toBe(true);
    // Consumed: a stray later mousedown gets the default, not a stale verdict.
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('refuses the compatibility mousedown of a touch that moved', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown(touch(100, 200));
    guard.pointerMove(touch(100, 200 + TAP_SLOP_PX + 1));
    guard.pointerMove(touch(100, 200));
    guard.pointerEnd(touch(100, 200));
    expect(guard.allowsMouseDown()).toBe(false);
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('refuses a mousedown while a touch is still down', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown(touch(0, 0));
    expect(guard.allowsMouseDown()).toBe(false);
  });

  it('refuses after the browser cancelled a touch', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown(touch(0, 0));
    guard.pointerCancel();
    expect(guard.allowsMouseDown()).toBe(false);
  });

  it('a cancel with no press in progress keeps the last verdict', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown(touch(0, 0));
    guard.pointerEnd(touch(0, 0));
    guard.pointerCancel();
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('treats a pen like a touch', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown({ pointerType: 'pen', clientX: 0, clientY: 0 });
    guard.pointerMove({ pointerType: 'pen', clientX: 40, clientY: 0 });
    guard.pointerEnd({ pointerType: 'pen', clientX: 40, clientY: 0 });
    expect(guard.allowsMouseDown()).toBe(false);
  });

  it('a new press replaces an unconsumed verdict', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown(touch(0, 0));
    guard.pointerMove(touch(50, 0));
    guard.pointerEnd(touch(50, 0));
    guard.pointerDown(touch(0, 0));
    guard.pointerEnd(touch(0, 0));
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('ignores the moves and lift of another pointer', () => {
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, null);
    guard.pointerDown(touch(0, 0, 1));
    guard.pointerMove(touch(80, 0, 2));
    guard.pointerEnd(touch(80, 0, 2));
    expect(guard.allowsMouseDown()).toBe(false);
    guard.pointerEnd(touch(0, 0, 1));
    expect(guard.allowsMouseDown()).toBe(true);
  });
});

describe('a gesture an ancestor captured mid-way', () => {
  // After DiffModal's container takes pointer capture (a pane drag or a
  // horizontal scroll past its dead zone) the viewer sees no more of the
  // gesture; the guard reads the rest from the window.
  it('still sees the moves and the lift, so the swipe does not select', () => {
    const win = new EventTarget();
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, win);
    guard.pointerDown(touch(100, 100, 1));
    win.dispatchEvent(pointerEvent('pointermove', 'touch', 140, 100));
    win.dispatchEvent(pointerEvent('pointerup', 'touch', 140, 100));
    expect(guard.allowsMouseDown()).toBe(false);
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('a tap whose lift was retargeted still selects', () => {
    const win = new EventTarget();
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, win);
    guard.pointerDown(touch(100, 100, 1));
    win.dispatchEvent(pointerEvent('pointerup', 'touch', 102, 101));
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('a lost capture with no lift cancels the press', () => {
    const win = new EventTarget();
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, win);
    guard.pointerDown(touch(0, 0, 1));
    win.dispatchEvent(pointerEvent('lostpointercapture', 'touch', 0, 0));
    expect(guard.allowsMouseDown()).toBe(false);
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('the lost capture that follows a lift keeps its verdict', () => {
    const win = new EventTarget();
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, win);
    guard.pointerDown(touch(0, 0, 1));
    win.dispatchEvent(pointerEvent('pointerup', 'touch', 0, 0));
    win.dispatchEvent(pointerEvent('lostpointercapture', 'touch', 0, 0));
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('listens only while a press is in progress, and destroy drops the listeners', () => {
    const win = new EventTarget();
    const add = vi.spyOn(win, 'addEventListener');
    const remove = vi.spyOn(win, 'removeEventListener');
    const guard = createTouchSelectionGuard(TAP_SLOP_PX, win);
    expect(add).not.toHaveBeenCalled();
    guard.pointerDown(touch(0, 0, 1));
    expect(add.mock.calls.map((c) => c[0]).sort()).toEqual([
      'lostpointercapture',
      'pointercancel',
      'pointermove',
      'pointerup',
    ]);
    win.dispatchEvent(pointerEvent('pointerup', 'touch', 0, 0));
    expect(remove).toHaveBeenCalledTimes(4);
    guard.pointerDown(touch(0, 0, 1));
    guard.destroy();
    expect(remove).toHaveBeenCalledTimes(8);
    // Nothing listens any more: a lift reaches no press.
    win.dispatchEvent(pointerEvent('pointerup', 'touch', 0, 0));
    expect(guard.allowsMouseDown()).toBe(true);
  });
});
