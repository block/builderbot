import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { observeHeightChanges } from './observeHeightChanges';

// vitest runs in node here; a stand-in ResizeObserver lets each test report
// the observed box's height by hand.
class FakeResizeObserver {
  static instances: FakeResizeObserver[] = [];
  observed: Element[] = [];
  disconnected = false;

  constructor(
    private readonly callback: (entries: Array<{ contentRect: { height: number } }>) => void
  ) {
    FakeResizeObserver.instances.push(this);
  }

  observe(el: Element): void {
    this.observed.push(el);
  }

  disconnect(): void {
    this.disconnected = true;
  }

  report(height: number): void {
    this.callback([{ contentRect: { height } }]);
  }
}

const box = {} as Element;

function observer(): FakeResizeObserver {
  return FakeResizeObserver.instances[FakeResizeObserver.instances.length - 1];
}

beforeEach(() => {
  FakeResizeObserver.instances = [];
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('observeHeightChanges', () => {
  it('reports a change between two heights, not the first observation', () => {
    const onChange = vi.fn();
    observeHeightChanges(box, onChange);
    expect(observer().observed).toEqual([box]);

    observer().report(600);
    expect(onChange).not.toHaveBeenCalled();

    // The keyboard opens over a full-screen dialog and shrinks the box.
    observer().report(270);
    expect(onChange).toHaveBeenCalledTimes(1);

    observer().report(270);
    expect(onChange).toHaveBeenCalledTimes(1);

    observer().report(600);
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it('ignores the box being detached and reattached', () => {
    // A pooled session pane is parked (detached, 0 high) and later adopted by
    // a dialog of another size, whose owner restores the scroll itself.
    const onChange = vi.fn();
    observeHeightChanges(box, onChange);
    observer().report(600);

    observer().report(0);
    observer().report(480);
    expect(onChange).not.toHaveBeenCalled();

    observer().report(300);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('disconnects on cleanup', () => {
    const stop = observeHeightChanges(box, () => {});
    stop();
    expect(observer().disconnected).toBe(true);
  });

  it('is a no-op without ResizeObserver', () => {
    vi.stubGlobal('ResizeObserver', undefined);
    const stop = observeHeightChanges(box, () => {});
    expect(FakeResizeObserver.instances).toHaveLength(0);
    stop();
  });
});
