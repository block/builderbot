// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createDiagramZoomController, type DiagramZoomController } from './diagramZoom';

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
    controller = createDiagramZoomController(viewportEl, contentEl, {
      minScale: 0.05,
      maxScale: 8,
      fitPadding: 0,
      doubleClickScale: 1.25,
      isEnabled: () => true,
      onTransform: (transform) => transforms.push(transform),
      onDraggingChange: () => {},
    });
    controller.reset();
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
    expect(currentScale()).toBeCloseTo(3);
    const zoomed = transforms.at(-1)!;

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
