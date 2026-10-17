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
    type: 'touchstart' | 'touchmove' | 'touchend',
    points: [number, number][]
  ) {
    const touches = points.map(([clientX, clientY], identifier) => ({
      identifier,
      clientX,
      clientY,
      target: viewportEl,
    }));
    const event = new TouchEvent(type, { bubbles: true, cancelable: true });
    Object.defineProperty(event, 'touches', { value: type === 'touchend' ? [] : touches });
    Object.defineProperty(event, 'changedTouches', { value: touches });
    viewportEl.dispatchEvent(event);
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
