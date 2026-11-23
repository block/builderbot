import { beforeEach, describe, expect, it, vi } from 'vitest';
import { guardTouchTaps, TAP_CLICK_WINDOW_MS, TAP_MAX_TRAVEL_PX } from './touchTapGuard';

// vitest runs in node, and a dispatched `Event` is never trusted, so the
// document is a stand-in that hands plain event objects to the capture
// listeners the guard registered. Positions are from the iOS 27.2 Safari
// trace of an accidental open: finger down at y=438, lifted at y=238.

type Listener = (event: unknown) => void;

class FakeDocument {
  private listeners = new Map<string, Set<Listener>>();
  captureFlags: unknown[] = [];

  addEventListener(type: string, listener: Listener, options?: unknown): void {
    this.captureFlags.push(options);
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(listener);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  listenerCount(): number {
    let count = 0;
    for (const set of this.listeners.values()) count += set.size;
    return count;
  }

  emit(type: string, event: object): void {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

let doc: FakeDocument;
let now: number;

function pointer(
  type: 'pointerdown' | 'pointerup' | 'pointercancel',
  y: number,
  fields: { x?: number; pointerId?: number; pointerType?: string; isPrimary?: boolean } = {}
): void {
  doc.emit(type, {
    pointerId: fields.pointerId ?? 1,
    pointerType: fields.pointerType ?? 'touch',
    isPrimary: fields.isPrimary ?? true,
    clientX: fields.x ?? 200,
    clientY: y,
    timeStamp: now,
  });
}

/** Deliver a click; returns whether the guard swallowed it. */
function click(isTrusted = true): boolean {
  const event = {
    isTrusted,
    timeStamp: now,
    preventDefault: vi.fn(),
    stopImmediatePropagation: vi.fn(),
  };
  doc.emit('click', event);
  const prevented = event.preventDefault.mock.calls.length > 0;
  expect(event.stopImmediatePropagation.mock.calls.length > 0).toBe(prevented);
  return prevented;
}

/** A touch from `fromY` to `toY`, lifted `durationMs` later. */
function touch(fromY: number, toY: number, durationMs = 60): void {
  pointer('pointerdown', fromY);
  now += durationMs;
  pointer('pointerup', toY);
}

beforeEach(() => {
  doc = new FakeDocument();
  now = 1000;
  guardTouchTaps(doc as unknown as EventTarget);
});

describe('guardTouchTaps', () => {
  it('swallows the click a flick ends in', () => {
    touch(438, 238);
    now += 10;
    expect(click()).toBe(true);
  });

  it('lets a plain tap click', () => {
    touch(438, 438);
    now += 10;
    expect(click()).toBe(false);
  });

  it('measures travel as a straight line, and only beyond the limit', () => {
    pointer('pointerdown', 400, { x: 100 });
    pointer('pointerup', 400 + TAP_MAX_TRAVEL_PX, { x: 100 });
    expect(click()).toBe(false);

    // 8 px across and 8 px down is 11.3 px of travel.
    pointer('pointerdown', 400, { x: 100 });
    pointer('pointerup', 408, { x: 108 });
    expect(click()).toBe(true);
  });

  it('swallows only one click per travelled touch', () => {
    touch(438, 238);
    expect(click()).toBe(true);
    expect(click()).toBe(false);
  });

  it('stops waiting once the click window has passed', () => {
    touch(438, 238);
    now += TAP_CLICK_WINDOW_MS + 1;
    expect(click()).toBe(false);
  });

  it('still swallows a click at the end of the window', () => {
    touch(438, 238);
    now += TAP_CLICK_WINDOW_MS;
    expect(click()).toBe(true);
  });

  it('stops waiting when the next press begins', () => {
    touch(438, 238);
    now += 100;
    touch(300, 300);
    expect(click()).toBe(false);
  });

  it('stops waiting when a mouse press begins', () => {
    touch(438, 238);
    pointer('pointerdown', 300, { pointerId: 9, pointerType: 'mouse' });
    pointer('pointerup', 300, { pointerId: 9, pointerType: 'mouse' });
    expect(click()).toBe(false);
  });

  it('forgets a touch the browser cancelled to scroll', () => {
    pointer('pointerdown', 438);
    pointer('pointercancel', 420);
    // A stray pointerup for the cancelled pointer must not arm the guard.
    pointer('pointerup', 238);
    expect(click()).toBe(false);
  });

  it('ignores a cancel for another pointer', () => {
    pointer('pointerdown', 438);
    pointer('pointercancel', 438, { pointerId: 2, isPrimary: false });
    pointer('pointerup', 238);
    expect(click()).toBe(true);
  });

  it('ignores mouse and pen drags', () => {
    for (const pointerType of ['mouse', 'pen']) {
      pointer('pointerdown', 438, { pointerType });
      pointer('pointerup', 238, { pointerType });
      expect(click()).toBe(false);
    }
  });

  it('ignores a second finger', () => {
    pointer('pointerdown', 438, { pointerId: 2, isPrimary: false });
    pointer('pointerup', 238, { pointerId: 2, isPrimary: false });
    expect(click()).toBe(false);
  });

  it("measures the primary touch against its own pointerup, not another finger's", () => {
    pointer('pointerdown', 438);
    pointer('pointerup', 100, { pointerId: 2, isPrimary: false });
    expect(click()).toBe(false);
    pointer('pointerup', 238);
    expect(click()).toBe(true);
  });

  it('passes a scripted click through and keeps waiting for the real one', () => {
    touch(438, 238);
    expect(click(false)).toBe(false);
    expect(click()).toBe(true);
  });

  it('listens on the capture phase and removes every listener on cleanup', () => {
    const other = new FakeDocument();
    const stop = guardTouchTaps(other as unknown as EventTarget);
    expect(other.captureFlags).toEqual([true, true, true, true]);
    expect(other.listenerCount()).toBe(4);
    stop();
    expect(other.listenerCount()).toBe(0);
  });

  it('is a no-op without a document', () => {
    expect(() => guardTouchTaps()()).not.toThrow();
  });
});
