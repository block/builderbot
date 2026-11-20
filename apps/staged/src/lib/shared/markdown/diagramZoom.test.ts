// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createDiagramZoomController, type DiagramZoomController } from './diagramZoom';

type ControllerOptions = Parameters<typeof createDiagramZoomController>[2];

describe('createDiagramZoomController', () => {
  let viewportEl: HTMLDivElement;
  let contentEl: HTMLDivElement;
  let controller: DiagramZoomController;
  let transforms: Array<{ scale: number; offsetX: number; offsetY: number }>;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-01-01T00:00:00Z'));

    viewportEl = document.createElement('div');
    contentEl = document.createElement('div');
    viewportEl.append(contentEl);
    document.body.append(viewportEl);

    Object.defineProperty(viewportEl, 'getBoundingClientRect', {
      configurable: true,
      value: () => ({
        width: 500,
        height: 400,
        top: 0,
        left: 0,
        right: 500,
        bottom: 400,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      }),
    });
    Object.defineProperty(viewportEl, 'clientWidth', { configurable: true, value: 500 });
    Object.defineProperty(viewportEl, 'clientHeight', { configurable: true, value: 400 });
    Object.defineProperty(contentEl, 'offsetWidth', { configurable: true, value: 100 });
    Object.defineProperty(contentEl, 'offsetHeight', { configurable: true, value: 100 });

    transforms = [];
    createController();
  });

  afterEach(() => {
    controller.destroy();
    viewportEl.remove();
    vi.useRealTimers();
  });

  it('zooms once for each repeated double-click pair', () => {
    dispatchClick(120, 140);
    expect(currentScale()).toBeCloseTo(1);

    vi.advanceTimersByTime(80);
    dispatchClick(121, 141);
    expect(currentScale()).toBeCloseTo(1.25);

    vi.advanceTimersByTime(80);
    dispatchClick(121, 141);
    expect(currentScale()).toBeCloseTo(1.25);

    vi.advanceTimersByTime(80);
    dispatchClick(122, 142);
    expect(currentScale()).toBeCloseTo(1.5625);
  });

  it('does not apply an extra zoom from the native dblclick event', () => {
    dispatchClick(120, 140);
    vi.advanceTimersByTime(80);
    dispatchClick(121, 141);

    viewportEl.dispatchEvent(
      new MouseEvent('dblclick', {
        bubbles: true,
        cancelable: true,
        clientX: 121,
        clientY: 141,
      })
    );

    expect(currentScale()).toBeCloseTo(1.25);
  });

  it('zooms and pans from a two-finger pinch', () => {
    // Fingers 40px apart, spreading to 120px apart around the same centre.
    dispatchTouch('touchstart', [
      [230, 200],
      [270, 200],
    ]);
    dispatchTouch('touchmove', [
      [190, 200],
      [310, 200],
    ]);
    // Zoom about the pinch centre (250, 200), which is (50, 50) in content space.
    expect(currentScale()).toBeCloseTo(3);
    const zoomed = transforms.at(-1)!;
    expect(zoomed.offsetX).toBeCloseTo(250 - 50 * 3);
    expect(zoomed.offsetY).toBeCloseTo(200 - 50 * 3);

    // Both fingers sliding together pans without changing the scale.
    dispatchTouch('touchmove', [
      [150, 170],
      [270, 170],
    ]);
    dispatchTouch('touchend', [
      [150, 170],
      [270, 170],
    ]);
    expect(currentScale()).toBeCloseTo(3);
    expect(transforms.at(-1)!.offsetX).toBeCloseTo(zoomed.offsetX - 40);
    expect(transforms.at(-1)!.offsetY).toBeCloseTo(zoomed.offsetY - 30);
  });

  it('pans with a single finger drag', () => {
    const before = transforms.at(-1)!;
    dispatchTouch('touchstart', [[200, 200]]);
    dispatchTouch('touchmove', [[260, 150]]);
    dispatchTouch('touchend', [[260, 150]]);

    expect(currentScale()).toBeCloseTo(before.scale);
    expect(transforms.at(-1)!.offsetX).toBeCloseTo(before.offsetX + 60);
    expect(transforms.at(-1)!.offsetY).toBeCloseTo(before.offsetY - 50);
  });

  it('pans by the wheel delta without a modifier key', () => {
    const before = transforms.at(-1)!;
    dispatchWheel(60, 40);

    expect(currentScale()).toBeCloseTo(before.scale);
    expect(transforms.at(-1)!.offsetX).toBeCloseTo(before.offsetX - 60);
    expect(transforms.at(-1)!.offsetY).toBeCloseTo(before.offsetY - 40);
  });

  it('double-clicks back to the fit once zoomed when toggling is enabled', () => {
    createController({ doubleClickTogglesFit: true });

    dispatchClick(120, 140);
    vi.advanceTimersByTime(80);
    dispatchClick(121, 141);
    expect(currentScale()).toBeCloseTo(1.25);

    vi.advanceTimersByTime(80);
    dispatchClick(121, 141);
    vi.advanceTimersByTime(80);
    dispatchClick(122, 142);
    expect(transforms.at(-1)).toEqual({ scale: 1, offsetX: 200, offsetY: 150 });
  });

  it('lets the scale floor follow a fit below the minimum scale', () => {
    // 100×16000 content fits the 400px-tall viewport at 0.025, under the 0.05 floor.
    Object.defineProperty(contentEl, 'offsetHeight', { configurable: true, value: 16000 });
    createController();
    expect(currentScale()).toBeCloseTo(0.025);

    controller.zoomBy(4);
    expect(currentScale()).toBeCloseTo(0.1);
    controller.zoomBy(0.1);
    expect(currentScale()).toBeCloseTo(0.025);
  });

  it('keeps the content within the viewport when constrained', () => {
    createController({ constrainToContent: true });

    // Smaller than the viewport: a drag leaves it centred.
    dispatchTouch('touchstart', [[200, 200]]);
    dispatchTouch('touchmove', [[260, 150]]);
    dispatchTouch('touchend', [[260, 150]]);
    expect(transforms.at(-1)).toEqual({ scale: 1, offsetX: 200, offsetY: 150 });

    // Larger than the viewport: it must keep covering the viewport.
    controller.zoomBy(8);
    dispatchTouch('touchstart', [[200, 200]]);
    dispatchTouch('touchmove', [[1200, 1200]]);
    dispatchTouch('touchend', [[1200, 1200]]);
    expect(transforms.at(-1)).toEqual({ scale: 8, offsetX: 0, offsetY: 0 });

    dispatchTouch('touchstart', [[200, 200]]);
    dispatchTouch('touchmove', [[-5000, -5000]]);
    dispatchTouch('touchend', [[-5000, -5000]]);
    expect(transforms.at(-1)).toEqual({ scale: 8, offsetX: 500 - 800, offsetY: 400 - 800 });

    // A wheel pan is clamped the same way, and still pans within the extent.
    dispatchWheel(-5000, -5000);
    expect(transforms.at(-1)).toEqual({ scale: 8, offsetX: 0, offsetY: 0 });
    dispatchWheel(60, 40);
    expect(transforms.at(-1)).toEqual({ scale: 8, offsetX: -60, offsetY: -40 });
    dispatchWheel(5000, 5000);
    expect(transforms.at(-1)).toEqual({ scale: 8, offsetX: 500 - 800, offsetY: 400 - 800 });
  });

  it('re-fits on resize only while the view is still at the fit', () => {
    Object.defineProperty(viewportEl, 'clientWidth', { configurable: true, value: 400 });
    controller.refit();
    expect(transforms.at(-1)).toEqual({ scale: 1, offsetX: 150, offsetY: 150 });

    controller.zoomBy(2);
    const zoomed = transforms.at(-1)!;
    Object.defineProperty(viewportEl, 'clientWidth', { configurable: true, value: 500 });
    controller.refit();
    expect(transforms.at(-1)).toEqual(zoomed);
  });

  it('places the content programmatically, outside the scale limits and the constraint', () => {
    createController({ constrainToContent: true });
    controller.setTransform({ scale: 0.01, offsetX: -40, offsetY: 900 });
    expect(transforms.at(-1)).toEqual({ scale: 0.01, offsetX: -40, offsetY: 900 });
  });

  it('settles a programmatic placement like an ordinary zoom', () => {
    createController({ constrainToContent: true });
    // Over the limit: brought back to it about the anchor, then kept covering the viewport.
    controller.setTransform({ scale: 16, offsetX: -700, offsetY: -500 });
    controller.settleTransform({ scale: 16, offsetX: -700, offsetY: -500 }, [100, 100]);
    expect(transforms.at(-1)).toEqual({ scale: 8, offsetX: -300, offsetY: -200 });

    // Within the limit but showing a gap at the left edge: only the gap closes.
    controller.settleTransform({ scale: 6, offsetX: 40, offsetY: -100 }, [250, 200]);
    expect(transforms.at(-1)).toEqual({ scale: 6, offsetX: 0, offsetY: -100 });
  });

  describe('pinch dismiss', () => {
    let progress: number[];
    let releases: boolean[];

    beforeEach(() => {
      progress = [];
      releases = [];
      createController({
        constrainToContent: true,
        pinchDismiss: {
          onProgress: (value) => progress.push(value),
          onRelease: (close) => releases.push(close),
        },
      });
    });

    // Two fingers on a horizontal line through (250, 200), `gap` px apart.
    const fingers = (gap: number, dx = 0, dy = 0): [number, number][] => [
      [250 - gap / 2 + dx, 200 + dy],
      [250 + gap / 2 + dx, 200 + dy],
    ];

    function pinch(gaps: number[], { stepMs = 200, holdMs = 300 } = {}) {
      dispatchTouch('touchstart', fingers(gaps[0]));
      for (const gap of gaps.slice(1)) {
        vi.advanceTimersByTime(stepMs);
        dispatchTouch('touchmove', fingers(gap));
      }
      vi.advanceTimersByTime(holdMs);
      dispatchTouch('touchend', fingers(gaps.at(-1)!));
      // d3-zoom's own double-tap window.
      vi.advanceTimersByTime(600);
    }

    it('follows the fingers below the fit and closes on release', () => {
      dispatchTouch('touchstart', fingers(200));
      vi.advanceTimersByTime(200);
      // Half the gap, and both fingers 30px right and 20px down.
      dispatchTouch('touchmove', fingers(100, 30, 20));
      expect(currentScale()).toBeCloseTo(0.5);
      // Unconstrained: the content centre stays under the pinch centre.
      expect(transforms.at(-1)!.offsetX).toBeCloseTo(280 - 50 * 0.5);
      expect(transforms.at(-1)!.offsetY).toBeCloseTo(220 - 50 * 0.5);
      expect(progress.at(-1)).toBe(1);
      expect(releases).toEqual([]);

      vi.advanceTimersByTime(300);
      dispatchTouch('touchend', fingers(100, 30, 20));
      expect(releases).toEqual([true]);
    });

    it('reports partial progress and does not close a small pinch-in', () => {
      pinch([200, 180]);
      expect(currentScale()).toBeCloseTo(0.9);
      expect(progress.at(-1)).toBeCloseTo(0.25);
      expect(releases).toEqual([false]);

      // The caller springs back with reset(); the next pinch-in is a close again.
      controller.reset();
      expect(transforms.at(-1)).toEqual({ scale: 1, offsetX: 200, offsetY: 150 });
      pinch([200, 100]);
      expect(releases).toEqual([false, true]);
    });

    it('closes a small pinch-in that is still moving fast when released', () => {
      pinch([200, 190, 180], { stepMs: 20, holdMs: 0 });
      expect(currentScale()).toBeCloseTo(0.9);
      expect(releases).toEqual([true]);
    });

    it('rubber-bands at the fit, without closing, when the pinch starts zoomed in', () => {
      controller.zoomBy(4);
      dispatchTouch('touchstart', fingers(200));
      vi.advanceTimersByTime(200);
      // The fingers ask for half the fit; the content gives a little.
      dispatchTouch('touchmove', fingers(25, 30, 20));
      const half = currentScale();
      expect(half).toBeLessThan(1);
      expect(half).toBeGreaterThan(0.85);
      // Centred, wherever the fingers are.
      expect(transforms.at(-1)!.offsetX).toBeCloseTo((500 - 100 * half) / 2);
      expect(transforms.at(-1)!.offsetY).toBeCloseTo((400 - 100 * half) / 2);

      // Much further gives only a little more, and never past the limit.
      vi.advanceTimersByTime(20);
      dispatchTouch('touchmove', fingers(1, 30, 20));
      expect(currentScale()).toBeLessThan(half);
      expect(currentScale()).toBeGreaterThan(0.8);
      expect(progress).toEqual([]);

      // Released, however far and fast: back to the fit, never a close.
      dispatchTouch('touchend', fingers(1, 30, 20));
      vi.advanceTimersByTime(600);
      expect(releases).toEqual([false]);
      controller.reset();
      expect(transforms.at(-1)).toEqual({ scale: 1, offsetX: 200, offsetY: 150 });

      // Now at the fit, so a second pinch-in closes.
      pinch([200, 100]);
      expect(releases).toEqual([false, true]);
    });

    it('leaves a zoom back out that stays above the fit alone', () => {
      controller.zoomBy(4);
      pinch([200, 100]);
      expect(currentScale()).toBeCloseTo(2);
      expect(releases).toEqual([]);
    });

    it('does not close when the touch sequence is cancelled', () => {
      dispatchTouch('touchstart', fingers(200));
      vi.advanceTimersByTime(200);
      dispatchTouch('touchmove', fingers(100));
      vi.advanceTimersByTime(300);
      dispatchTouch('touchcancel', fingers(100));
      expect(releases).toEqual([false]);
    });

    it('leaves a pinch-out from the fit as an ordinary zoom', () => {
      pinch([100, 300]);
      expect(currentScale()).toBeCloseTo(3);
      expect(progress.every((value) => value === 0)).toBe(true);
      expect(releases).toEqual([]);
    });

    it('does not report a pinch that went below the fit and came back', () => {
      pinch([200, 100, 200]);
      expect(transforms.at(-1)).toEqual({ scale: 1, offsetX: 200, offsetY: 150 });
      expect(progress.at(-1)).toBe(0);
      expect(releases).toEqual([]);
    });

    it('keeps the wheel floor below the fit', () => {
      controller.zoomBy(0.5);
      expect(currentScale()).toBeCloseTo(0.5);
      expect(releases).toEqual([]);
    });
  });

  it('lets a pinch-in rest below the fit without pinch dismiss', () => {
    createController({ constrainToContent: true });
    dispatchTouch('touchstart', [
      [150, 200],
      [350, 200],
    ]);
    dispatchTouch('touchmove', [
      [200, 200],
      [300, 200],
    ]);
    dispatchTouch('touchend', [
      [200, 200],
      [300, 200],
    ]);
    expect(transforms.at(-1)).toEqual({ scale: 0.5, offsetX: 225, offsetY: 175 });
  });

  function createController(overrides: Partial<ControllerOptions> = {}) {
    controller?.destroy();
    controller = createDiagramZoomController(viewportEl, contentEl, {
      minScale: 0.05,
      maxScale: 8,
      fitPadding: 0,
      doubleClickScale: 1.25,
      isEnabled: () => true,
      onTransform: (transform) => transforms.push(transform),
      onDraggingChange: () => {},
      ...overrides,
    });
    controller.reset();
  }

  // jsdom has no `Touch` constructor; d3-zoom only reads identifier/clientX/clientY.
  function dispatchTouch(
    type: 'touchstart' | 'touchmove' | 'touchend' | 'touchcancel',
    points: [number, number][]
  ) {
    const touches = points.map(([clientX, clientY], identifier) => ({
      identifier,
      clientX,
      clientY,
      target: viewportEl,
    }));
    const event = new TouchEvent(type, { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'touches', {
      value: type === 'touchend' || type === 'touchcancel' ? [] : touches,
    });
    Object.defineProperty(event, 'changedTouches', { value: touches });
    viewportEl.dispatchEvent(event);
  }

  function dispatchWheel(deltaX: number, deltaY: number) {
    viewportEl.dispatchEvent(
      new WheelEvent('wheel', {
        bubbles: true,
        cancelable: true,
        deltaX,
        deltaY,
        deltaMode: WheelEvent.DOM_DELTA_PIXEL,
      })
    );
  }

  function dispatchClick(clientX: number, clientY: number) {
    viewportEl.dispatchEvent(
      new MouseEvent('click', {
        bubbles: true,
        cancelable: true,
        clientX,
        clientY,
      })
    );
  }

  function currentScale(): number {
    return transforms.at(-1)?.scale ?? 0;
  }
});
