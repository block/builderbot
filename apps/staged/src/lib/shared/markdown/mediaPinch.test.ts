// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderMarkdown } from './renderMarkdown';
import {
  MediaPinchGesture,
  containedImageRect,
  mediaPinchOpen,
  pinchOpenProgress,
  pinchTransform,
  transformOverRect,
  visibleImagePlace,
  visibleImageRect,
  type OpenMarkdownMedia,
  type PinchFrame,
} from './mediaPinch';
vi.mock('../../transport', () => ({ isTauri: false }));
const id = '3f2a9c1e-1234-4567-8123-123456789abc';

describe('pinch geometry', () => {
  it('finds the picture inside a letterboxed image box', () => {
    expect(
      containedImageRect(
        { left: 10, top: 20, width: 400, height: 100 },
        { width: 800, height: 400 }
      )
    ).toEqual({ left: 110, top: 20, width: 200, height: 100 });
    const box = { left: 0, top: 0, width: 100, height: 50 };
    expect(containedImageRect(box, { width: 0, height: 0 })).toBe(box);
  });

  it('lays content over a rectangle on screen', () => {
    expect(
      transformOverRect(
        { left: 25, top: 207, width: 352, height: 220 },
        { left: 0, top: 47 },
        { width: 1600, height: 1000 }
      )
    ).toEqual({ scale: 0.22, offsetX: 25, offsetY: 160 });
  });

  it('keeps what was under the fingers under them', () => {
    const origin = { scale: 0.2, offsetX: 20, offsetY: 100 };
    const stage = { left: 0, top: 50 };
    const start = { x: 120, y: 250 };
    // Content point under the starting midpoint.
    const contentX = (start.x - stage.left - origin.offsetX) / origin.scale;
    const contentY = (start.y - stage.top - origin.offsetY) / origin.scale;

    expect(pinchTransform(origin, start, { scale: 1, ...start }, stage)).toEqual(origin);

    const moved = pinchTransform(origin, start, { scale: 2.5, x: 180, y: 300 }, stage);
    expect(moved.scale).toBeCloseTo(0.5);
    expect(stage.left + moved.offsetX + contentX * moved.scale).toBeCloseTo(180);
    expect(stage.top + moved.offsetY + contentY * moved.scale).toBeCloseTo(300);
  });

  it('maps the spread to backdrop progress', () => {
    expect(pinchOpenProgress(0.8)).toBe(0);
    expect(pinchOpenProgress(1)).toBe(0);
    expect(pinchOpenProgress(1.25)).toBeCloseTo(0.5);
    expect(pinchOpenProgress(3)).toBe(1);
  });
});

describe('MediaPinchGesture', () => {
  const rect = { left: 0, top: 0, width: 100, height: 100 };
  const frame = (scale: number): PinchFrame => ({ scale, x: 50, y: 50 });

  it('opens when released well spread and cancels when released early', () => {
    const wide = new MediaPinchGesture({ x: 50, y: 50 }, rect);
    wide.update(frame(1.3), 0);
    wide.end(false, 1000);
    expect(wide.outcome).toBe('open');

    const early = new MediaPinchGesture({ x: 50, y: 50 }, rect);
    early.update(frame(1.1), 0);
    early.end(false, 1000);
    expect(early.outcome).toBe('cancel');
  });

  it('opens a small spread that is still moving fast, but never a cancelled touch', () => {
    const flick = new MediaPinchGesture({ x: 50, y: 50 }, rect);
    flick.update(frame(1.02), 0);
    flick.update(frame(1.1), 40);
    flick.end(false, 45);
    expect(flick.outcome).toBe('open');

    const cancelled = new MediaPinchGesture({ x: 50, y: 50 }, rect);
    cancelled.update(frame(2), 0);
    cancelled.end(true, 10);
    expect(cancelled.outcome).toBe('cancel');
  });

  it('catches a late listener up with the latest frame and the end', () => {
    const gesture = new MediaPinchGesture({ x: 50, y: 50 }, rect);
    const early = { frame: vi.fn(), end: vi.fn() };
    const unsubscribe = gesture.subscribe(early);
    expect(early.frame).toHaveBeenLastCalledWith({ scale: 1, x: 50, y: 50 });

    gesture.update(frame(1.4), 0);
    expect(early.frame).toHaveBeenLastCalledWith(frame(1.4));
    unsubscribe();
    gesture.update(frame(1.6), 10);
    expect(early.frame).toHaveBeenCalledTimes(2);

    gesture.end(false, 500);
    const late = { frame: vi.fn(), end: vi.fn() };
    gesture.subscribe(late);
    expect(late.frame).toHaveBeenCalledWith(frame(1.6));
    expect(late.end).toHaveBeenCalledWith(true);
    expect(early.end).not.toHaveBeenCalled();
  });
});

describe('mediaPinchOpen', () => {
  let host: HTMLDivElement;
  let image: HTMLImageElement;
  let text: HTMLElement;
  let opened: OpenMarkdownMedia[];
  let detach: void | (() => void);

  beforeEach(() => {
    host = document.createElement('div');
    host.className = 'markdown-content';
    host.innerHTML = renderMarkdown(`Before\n\n![Plan](staged-media://${id}.png)\n\nAfter`);
    document.body.append(host);
    image = host.querySelector('img')!;
    text = host.querySelector('p')!;
    Object.defineProperty(image, 'complete', { configurable: true, value: true });
    Object.defineProperty(image, 'naturalWidth', { configurable: true, value: 1600 });
    Object.defineProperty(image, 'naturalHeight', { configurable: true, value: 1000 });
    image.getBoundingClientRect = () => new DOMRect(25, 200, 352, 220);
    opened = [];
    detach = mediaPinchOpen((media) => opened.push(media))(host);
  });

  afterEach(() => {
    if (detach) detach();
    host.remove();
  });

  // x, y, the element the finger landed on (the image by default), identifier.
  type Point = [number, number, Element?, number?];

  /** Dispatched on `at`, or else on the first finger's element. */
  function touch(type: string, points: Point[], cancelable = true, at?: Element): TouchEvent {
    const touches = points.map(([clientX, clientY, target, identifier], index) => ({
      identifier: identifier ?? index,
      clientX,
      clientY,
      target: target ?? image,
    }));
    const event = new TouchEvent(type, { bubbles: true, cancelable });
    Object.defineProperty(event, 'touches', { value: touches });
    (at ?? points[0]?.[2] ?? image).dispatchEvent(event);
    return event;
  }

  it('opens the image once the fingers have spread, then feeds the gesture', () => {
    touch('touchstart', [
      [180, 300],
      [220, 300],
    ]);
    // Not spread far enough yet; the move is still kept from the browser.
    const small = touch('touchmove', [
      [178, 300],
      [222, 300],
    ]);
    expect(small.defaultPrevented).toBe(true);
    expect(opened).toEqual([]);

    touch('touchmove', [
      [170, 300],
      [230, 300],
    ]);
    expect(opened).toHaveLength(1);
    const media = opened[0];
    expect(media).toMatchObject({ id, ext: 'png', alt: 'Plan' });
    expect(media.originImage).toBe(image);
    const gesture = media.gesture!;
    expect(gesture.start).toEqual({ x: 200, y: 300 });
    expect(gesture.startRect).toEqual({ left: 25, top: 200, width: 352, height: 220 });
    expect(gesture.latest.scale).toBe(1);

    // Scale is measured from the moment of recognition (60px apart).
    touch('touchmove', [
      [150, 310],
      [270, 310],
    ]);
    expect(gesture.latest).toEqual({ scale: 2, x: 210, y: 310 });

    touch('touchend', []);
    expect(gesture.outcome).toBe('open');
    expect(opened).toHaveLength(1);
  });

  it('cancels when the fingers lift early', () => {
    touch('touchstart', [
      [180, 300],
      [220, 300],
    ]);
    touch('touchmove', [
      [174, 300],
      [226, 300],
    ]);
    touch('touchmove', [
      [173, 300],
      [227, 300],
    ]);
    const gesture = opened[0].gesture!;
    expect(gesture.latest.scale).toBeCloseTo(54 / 52);
    // One finger lifts: the gesture is over, and the other finger does nothing more.
    touch('touchend', [[173, 300]]);
    expect(gesture.outcome).toBe('cancel');
    const after = touch('touchmove', [[100, 300]]);
    expect(after.defaultPrevented).toBe(true);
    touch('touchend', []);
    expect(touch('touchmove', [[100, 300]]).defaultPrevented).toBe(false);
  });

  it('takes a pinch with only one finger on the image', () => {
    touch('touchstart', [
      [100, 150, text],
      [200, 300, image],
    ]);
    touch('touchmove', [
      [90, 140, text],
      [220, 320, image],
    ]);
    expect(opened).toHaveLength(1);
    expect(opened[0].originImage).toBe(image);
  });

  it('swallows a pinch-in without opening', () => {
    touch('touchstart', [
      [100, 300],
      [300, 300],
    ]);
    const move = touch('touchmove', [
      [150, 300],
      [250, 300],
    ]);
    expect(move.defaultPrevented).toBe(true);
    touch('touchend', []);
    expect(opened).toEqual([]);
  });

  it('leaves one finger, pinches off the image, and scrolls in progress alone', () => {
    touch('touchstart', [[200, 300]]);
    expect(touch('touchmove', [[200, 250]]).defaultPrevented).toBe(false);
    touch('touchend', []);

    touch('touchstart', [
      [100, 150, text],
      [200, 150, text],
    ]);
    expect(
      touch('touchmove', [
        [50, 150, text],
        [250, 150, text],
      ]).defaultPrevented
    ).toBe(false);
    touch('touchend', []);

    // The second finger lands while the first is already scrolling the pane.
    touch(
      'touchstart',
      [
        [180, 300],
        [220, 300],
      ],
      false
    );
    touch('touchmove', [
      [100, 300],
      [300, 300],
    ]);
    expect(opened).toEqual([]);
  });

  it('sees a finger outside the container lift, so the next drag scrolls', () => {
    const outside = document.createElement('header');
    document.body.append(outside);
    // The finger outside landed first; the second, on the image, makes two.
    touch(
      'touchstart',
      [
        [200, 20, outside],
        [200, 300, image],
      ],
      true,
      image
    );
    // The finger on the image lifts first, then the one outside: that end
    // goes to the outside element and never bubbles through the container.
    touch('touchend', [[200, 20, outside]], true, image);
    touch('touchend', [], true, outside);
    expect(touch('touchmove', [[200, 250]]).defaultPrevented).toBe(false);

    // And nothing is left listening outside.
    expect(touch('touchmove', [[200, 20, outside]]).defaultPrevented).toBe(false);
    outside.remove();
  });

  it('follows a finger whose element is removed mid-gesture', () => {
    touch('touchstart', [
      [180, 300],
      [220, 300],
    ]);
    touch('touchmove', [
      [170, 300],
      [230, 300],
    ]);
    const gesture = opened[0].gesture!;

    // A re-render replaces the markdown: the touches stay with the old image,
    // whose events no longer reach the container.
    image.closest('.markdown-media')!.remove();
    const move = touch('touchmove', [
      [140, 300],
      [260, 300],
    ]);
    expect(move.defaultPrevented).toBe(true);
    expect(gesture.latest.scale).toBe(2);
    touch('touchend', []);
    expect(gesture.outcome).toBe('open');
    expect(touch('touchmove', [[200, 250, text]]).defaultPrevented).toBe(false);
  });

  it('starts afresh when the last fingers went without an end being seen', () => {
    touch('touchstart', [
      [180, 300, image, 1],
      [220, 300, image, 2],
    ]);
    touch('touchmove', [
      [170, 300, image, 1],
      [230, 300, image, 2],
    ]);
    const gesture = opened[0].gesture!;
    // No touchend arrives. A new finger then lands.
    touch('touchstart', [[200, 150, text, 3]]);
    expect(gesture.outcome).toBe('cancel');
    expect(touch('touchmove', [[200, 100, text, 3]]).defaultPrevented).toBe(false);
  });

  it('ends the gesture when the attachment is removed mid-pinch', () => {
    touch('touchstart', [
      [180, 300],
      [220, 300],
    ]);
    touch('touchmove', [
      [100, 300],
      [300, 300],
    ]);
    const gesture = opened[0].gesture!;
    if (detach) detach();
    detach = undefined;
    expect(gesture.outcome).toBe('cancel');
  });
});

describe('visibleImageRect', () => {
  it('is null for an image that is gone or scrolled out of view', () => {
    const pane = document.createElement('div');
    pane.style.overflowY = 'auto';
    const image = document.createElement('img');
    pane.append(image);
    Object.defineProperty(image, 'naturalWidth', { configurable: true, value: 200 });
    Object.defineProperty(image, 'naturalHeight', { configurable: true, value: 100 });
    pane.getBoundingClientRect = () => new DOMRect(0, 100, 400, 500);

    image.getBoundingClientRect = () => new DOMRect(20, 150, 200, 100);
    expect(visibleImageRect(image)).toBeNull();
    expect(visibleImageRect(null)).toBeNull();

    document.body.append(pane);
    expect(visibleImageRect(image)).toEqual({ left: 20, top: 150, width: 200, height: 100 });

    // Above the pane's top edge, though still inside the window.
    image.getBoundingClientRect = () => new DOMRect(20, -20, 200, 100);
    expect(visibleImageRect(image)).toBeNull();
    pane.remove();
  });

  it('reports the area a partly hidden image shows through', () => {
    const pane = document.createElement('div');
    pane.style.overflowY = 'auto';
    const image = document.createElement('img');
    pane.append(image);
    document.body.append(pane);
    Object.defineProperty(image, 'naturalWidth', { configurable: true, value: 200 });
    Object.defineProperty(image, 'naturalHeight', { configurable: true, value: 100 });
    pane.getBoundingClientRect = () => new DOMRect(0, 100, 400, 500);

    image.getBoundingClientRect = () => new DOMRect(20, 150, 200, 100);
    expect(visibleImagePlace(image)).toEqual({
      rect: { left: 20, top: 150, width: 200, height: 100 },
      clip: null,
    });

    // Its top 60px are above the pane's top edge.
    image.getBoundingClientRect = () => new DOMRect(20, 40, 200, 100);
    expect(visibleImagePlace(image)).toEqual({
      rect: { left: 20, top: 40, width: 200, height: 100 },
      clip: { left: 0, top: 100, width: 400, height: 500 },
    });
    pane.remove();
  });
});
