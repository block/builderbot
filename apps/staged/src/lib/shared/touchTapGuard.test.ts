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

/** A touch that lifts `dx` px across and `dy` px down from where it landed. */
function travel(dx: number, dy: number, durationMs: number): void {
  pointer('pointerdown', 438, { x: 200 });
  now += durationMs;
  pointer('pointerup', 438 + dy, { x: 200 + dx });
}

/** Whether the click of a touch that travelled `px` in `durationMs` is swallowed, up and sideways. */
function swallowed(px: number, durationMs: number): boolean[] {
  travel(0, -px, durationMs);
  const vertical = click();
  travel(px, 0, durationMs);
  return [vertical, click()];
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
    travel(0, TAP_MAX_TRAVEL_PX, 60);
    expect(click()).toBe(false);
    travel(0, TAP_MAX_TRAVEL_PX + 1, 60);
    expect(click()).toBe(true);

    // 30 px across and 30 px down is 42.4 px of travel; 32 and 32 is 45.3 px.
    travel(30, 30, 60);
    expect(click()).toBe(false);
    travel(32, 32, 60);
    expect(click()).toBe(true);
  });

  // The corners the rule has to hold at once: a pressing thumb rolls up to
  // about 30 px, quickly or slowly, and a touch that went far was no tap
  // however long it took.
  describe('the corners of the rule', () => {
    it('passes a 25 px roll lifted after 120 ms', () => {
      expect(swallowed(25, 120)).toEqual([false, false]);
    });

    it('passes a 30 px roll, quick or slow', () => {
      for (const durationMs of [30, 60, 120, 200, 400]) {
        expect(swallowed(30, durationMs), `${durationMs} ms`).toEqual([false, false]);
      }
    });

    it('swallows a 60 px flick lifted after 30 to 100 ms', () => {
      for (const durationMs of [30, 50, 80, 100]) {
        expect(swallowed(60, durationMs), `${durationMs} ms`).toEqual([true, true]);
      }
    });

    it('swallows an 80 px touch lifted after 250 ms', () => {
      expect(swallowed(80, 250)).toEqual([true, true]);
    });

    it('passes a press that rolls 10 to 20 px at any speed', () => {
      for (const px of [10, 15, 20]) {
        for (const durationMs of [20, 60, 120, 250, 1000]) {
          expect(swallowed(px, durationMs), `${px} px in ${durationMs} ms`).toEqual([false, false]);
        }
      }
    });

    it('swallows a far touch however slow', () => {
      for (const [px, durationMs] of [
        [60, 200],
        [100, 400],
        [200, 400],
        [200, 3000],
      ]) {
        expect(swallowed(px, durationMs), `${px} px in ${durationMs} ms`).toEqual([true, true]);
      }
    });
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
