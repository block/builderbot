import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { observeSizeChanges } from './observeSizeChanges';

// vitest runs in node here; a stand-in ResizeObserver lets each test report
// the observed box's size by hand.
class FakeResizeObserver {
  static instances: FakeResizeObserver[] = [];
  observed: Element[] = [];
  disconnected = false;

  constructor(
    private readonly callback: (
      entries: Array<{ contentRect: { width: number; height: number } }>
    ) => void
  ) {
    FakeResizeObserver.instances.push(this);
  }

  observe(el: Element): void {
    this.observed.push(el);
  }

  disconnect(): void {
    this.disconnected = true;
  }

  report(width: number, height: number): void {
    this.callback([{ contentRect: { width, height } }]);
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

describe('observeSizeChanges', () => {
  it('reports a change of height, not the first observation', () => {
    const onChange = vi.fn();
    observeSizeChanges(box, onChange);
    expect(observer().observed).toEqual([box]);

    observer().report(390, 600);
    expect(onChange).not.toHaveBeenCalled();

    // The keyboard opens over a full-screen dialog and shrinks the box.
    observer().report(390, 270);
    expect(onChange).toHaveBeenCalledTimes(1);

    observer().report(390, 270);
    expect(onChange).toHaveBeenCalledTimes(1);

    observer().report(390, 600);
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it('reports a change of width', () => {
    // Rotating the phone, or a dialog resized sideways: the transcript
    // reflows and its scroll height changes with the box's height unchanged.
    const onChange = vi.fn();
    observeSizeChanges(box, onChange);
    observer().report(390, 600);

    observer().report(844, 600);
    expect(onChange).toHaveBeenCalledTimes(1);

    observer().report(844, 600);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('ignores the box being detached and reattached', () => {
    // A pooled session pane is parked (detached, 0×0) and later adopted by
    // a dialog of another size, whose owner restores the scroll itself.
    const onChange = vi.fn();
    observeSizeChanges(box, onChange);
    observer().report(390, 600);

    observer().report(0, 0);
    observer().report(720, 480);
    expect(onChange).not.toHaveBeenCalled();

    observer().report(720, 300);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('ignores changes to or from a size with one dimension 0', () => {
    const onChange = vi.fn();
    observeSizeChanges(box, onChange);
    observer().report(390, 600);

    observer().report(0, 600);
    observer().report(390, 600);
    observer().report(390, 0);
    observer().report(390, 600);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('disconnects on cleanup', () => {
    const stop = observeSizeChanges(box, () => {});
    stop();
    expect(observer().disconnected).toBe(true);
  });

  it('is a no-op without ResizeObserver', () => {
    vi.stubGlobal('ResizeObserver', undefined);
    const stop = observeSizeChanges(box, () => {});
    expect(FakeResizeObserver.instances).toHaveLength(0);
    stop();
  });
});
