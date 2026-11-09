import { describe, expect, it } from 'vitest';
import { createTouchSelectionGuard, TAP_SLOP_PX } from './touchSelectionGuard';

function touch(x: number, y: number) {
  return { pointerType: 'touch', clientX: x, clientY: y };
}

function mouse(x: number, y: number) {
  return { pointerType: 'mouse', clientX: x, clientY: y };
}

describe('createTouchSelectionGuard', () => {
  it('allows a mousedown with no pointer events before it', () => {
    expect(createTouchSelectionGuard().allowsMouseDown()).toBe(true);
  });

  it('allows a mouse press while it is down', () => {
    const guard = createTouchSelectionGuard();
    guard.pointerDown(mouse(10, 10));
    expect(guard.allowsMouseDown()).toBe(true);
    guard.pointerEnd(mouse(10, 10));
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('allows the compatibility mousedown of a touch that stayed put, once', () => {
    const guard = createTouchSelectionGuard();
    guard.pointerDown(touch(100, 200));
    guard.pointerMove(touch(100 + TAP_SLOP_PX, 200 - TAP_SLOP_PX));
    guard.pointerEnd(touch(100 + TAP_SLOP_PX, 200 - TAP_SLOP_PX));
    expect(guard.allowsMouseDown()).toBe(true);
    // Consumed: a stray later mousedown gets the default, not a stale verdict.
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('refuses the compatibility mousedown of a touch that moved', () => {
    const guard = createTouchSelectionGuard();
    guard.pointerDown(touch(100, 200));
    guard.pointerMove(touch(100, 200 + TAP_SLOP_PX + 1));
    guard.pointerMove(touch(100, 200));
    guard.pointerEnd(touch(100, 200));
    expect(guard.allowsMouseDown()).toBe(false);
    expect(guard.allowsMouseDown()).toBe(true);
  });

  it('refuses a mousedown while a touch is still down', () => {
    const guard = createTouchSelectionGuard();
    guard.pointerDown(touch(0, 0));
    expect(guard.allowsMouseDown()).toBe(false);
  });

  it('refuses after the browser cancelled a touch', () => {
    const guard = createTouchSelectionGuard();
    guard.pointerDown(touch(0, 0));
    guard.pointerCancel();
    expect(guard.allowsMouseDown()).toBe(false);
  });

  it('treats a pen like a touch', () => {
    const guard = createTouchSelectionGuard();
    guard.pointerDown({ pointerType: 'pen', clientX: 0, clientY: 0 });
    guard.pointerMove({ pointerType: 'pen', clientX: 40, clientY: 0 });
    guard.pointerEnd({ pointerType: 'pen', clientX: 40, clientY: 0 });
    expect(guard.allowsMouseDown()).toBe(false);
  });

  it('a new press replaces an unconsumed verdict', () => {
    const guard = createTouchSelectionGuard();
    guard.pointerDown(touch(0, 0));
    guard.pointerMove(touch(50, 0));
    guard.pointerEnd(touch(50, 0));
    guard.pointerDown(touch(0, 0));
    guard.pointerEnd(touch(0, 0));
    expect(guard.allowsMouseDown()).toBe(true);
  });
});
