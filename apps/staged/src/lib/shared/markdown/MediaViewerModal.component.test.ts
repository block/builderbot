import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import MediaViewerModal from './MediaViewerModal.svelte';
import { MediaPinchGesture } from './mediaPinch';

// The viewer under the real dialog primitive and the real zoom controller,
// driven by touch events on its stage. What is covered here is where the
// picture goes when a pinch ends: back to the inline image, cut off where
// that image is, shrinking away when there is nowhere to return to, and
// staying zoomed after an opening pinch carried past the fit. jsdom lays
// nothing out, so the stage and the picture are given their boxes by hand:
//
//   window 1024×768, stage 400×600 at (0, 50), picture 800×400
//   fit: scale 0.46 at (16, 208), the picture's centre on the stage's centre

const STAGE = new DOMRect(0, 50, 400, 600);
const NATURAL = { width: 800, height: 400 };
const FIT = { scale: 0.46, x: 16, y: 208 };
const SETTLE_MS = 300;

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;
let pane: HTMLElement;
let origin: HTMLImageElement;
let onClose: ReturnType<typeof vi.fn<() => void>>;
let stage: HTMLElement;
let surface: HTMLElement;
let animate: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    }
  );
  target = document.createElement('div');
  // The inline image, in the pane the note scrolls in.
  pane = document.createElement('div');
  pane.style.overflowY = 'auto';
  pane.getBoundingClientRect = () => new DOMRect(0, 100, 400, 500);
  origin = document.createElement('img');
  Object.defineProperty(origin, 'naturalWidth', { configurable: true, value: NATURAL.width });
  Object.defineProperty(origin, 'naturalHeight', { configurable: true, value: NATURAL.height });
  placeOrigin(40, 300);
  pane.append(origin);
  document.body.append(pane, target);
  onClose = vi.fn<() => void>();
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  target.remove();
  pane.remove();
  vi.unstubAllGlobals();
});

/** The inline image is 200×100 on screen, a quarter of the picture's size. */
function placeOrigin(left: number, top: number) {
  origin.getBoundingClientRect = () => new DOMRect(left, top, 200, 100);
}

async function render(gesture: MediaPinchGesture | null = null) {
  component = mount(MediaViewerModal, {
    target,
    props: { open: true, src: 'plan.png', title: 'Plan', onClose, originImage: origin, gesture },
  });
  flushSync();
  await tick();
  stage = document.querySelector<HTMLElement>('.viewer-stage')!;
  surface = document.querySelector<HTMLElement>('.image-surface')!;
  if (!stage || !surface) throw new Error('viewer not rendered');
  stage.getBoundingClientRect = () => STAGE;
  Object.defineProperty(stage, 'clientWidth', { configurable: true, value: STAGE.width });
  Object.defineProperty(stage, 'clientHeight', { configurable: true, value: STAGE.height });
  Object.defineProperty(surface, 'offsetWidth', { configurable: true, value: NATURAL.width });
  Object.defineProperty(surface, 'offsetHeight', { configurable: true, value: NATURAL.height });
  animate = vi.fn();
  stage.animate = animate as unknown as HTMLElement['animate'];

  const img = surface.querySelector('img')!;
  Object.defineProperty(img, 'naturalWidth', { configurable: true, value: NATURAL.width });
  Object.defineProperty(img, 'naturalHeight', { configurable: true, value: NATURAL.height });
  img.dispatchEvent(new Event('load'));
  flushSync();
  await tick();
}

/** The picture's transform as scale and offset within the stage. */
function placement() {
  const match = /matrix\(([^)]+)\)/.exec(surface.style.transform);
  if (!match) throw new Error(`no transform: ${surface.getAttribute('style')}`);
  const [scale, , , , x, y] = match[1].split(',').map(Number);
  return { scale, x, y };
}

function expectPlacement(expected: { scale: number; x: number; y: number }) {
  const actual = placement();
  expect(actual.scale).toBeCloseTo(expected.scale, 4);
  expect(actual.x).toBeCloseTo(expected.x, 2);
  expect(actual.y).toBeCloseTo(expected.y, 2);
}

// Two fingers on a horizontal line through the stage's centre, `gap` px apart.
function touch(type: 'touchstart' | 'touchmove' | 'touchend' | 'touchcancel', gap: number) {
  const touches = [200 - gap / 2, 200 + gap / 2].map((clientX, identifier) => ({
    identifier,
    clientX,
    clientY: 350,
    target: stage,
  }));
  const event = new TouchEvent(type, { bubbles: true, cancelable: true });
  const lifted = type === 'touchend' || type === 'touchcancel';
  Object.defineProperty(event, 'touches', { value: lifted ? [] : touches });
  Object.defineProperty(event, 'changedTouches', { value: touches });
  stage.dispatchEvent(event);
}

/** Pinch from the fit down to `ratio` of it and let go. */
function pinchIn(ratio: number, end: 'touchend' | 'touchcancel' = 'touchend') {
  touch('touchstart', 200);
  touch('touchmove', 200 * ratio);
  flushSync();
  expectPlacement({
    scale: FIT.scale * ratio,
    x: 200 - (NATURAL.width * FIT.scale * ratio) / 2,
    y: 300 - (NATURAL.height * FIT.scale * ratio) / 2,
  });
  touch(end, 200 * ratio);
  flushSync();
}

async function settled() {
  await new Promise((resolve) => setTimeout(resolve, SETTLE_MS));
  flushSync();
}

describe('MediaViewerModal pinch close', () => {
  it('opens at the fit', async () => {
    await render();
    expectPlacement(FIT);
    expect(surface.style.opacity).toBe('');
  });

  it('returns the picture to the inline image', async () => {
    await render();
    pinchIn(0.4);

    // Laid exactly over the image: 200px wide at (40, 300) on screen.
    expectPlacement({ scale: 0.25, x: 40, y: 300 - STAGE.top });
    expect(surface.style.opacity).toBe('');
    expect(animate).not.toHaveBeenCalled();
    // The picture stands in for the inline image while it is in flight.
    expect(origin.style.visibility).toBe('hidden');
    expect(onClose).not.toHaveBeenCalled();

    await settled();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('shrinks and fades when the inline image is scrolled out of view', async () => {
    await render();
    // Above the top edge of the pane it scrolls in.
    placeOrigin(40, -40);
    pinchIn(0.4);

    // 0.6 of where the fingers left it, about its own centre.
    const scale = FIT.scale * 0.4 * 0.6;
    expectPlacement({
      scale,
      x: 200 - (NATURAL.width * scale) / 2,
      y: 300 - (NATURAL.height * scale) / 2,
    });
    expect(surface.style.opacity).toBe('0');
    expect(animate).not.toHaveBeenCalled();

    await settled();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('shrinks and fades when the inline image is gone', async () => {
    await render();
    origin.remove();
    pinchIn(0.4);

    expectPlacement({ scale: FIT.scale * 0.4 * 0.6, x: 200 - 44.16, y: 300 - 22.08 });
    expect(surface.style.opacity).toBe('0');
    await settled();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('cuts the returning picture off where a partly hidden image is', async () => {
    await render();
    // The image's top 60px are above the pane's top edge.
    placeOrigin(40, 40);
    pinchIn(0.4);

    // The whole picture goes back, hidden part included ...
    expectPlacement({ scale: 0.25, x: 40, y: 40 - STAGE.top });
    expect(surface.style.opacity).toBe('');
    // ... behind a clip that closes in from the window's edges to the pane.
    expect(animate).toHaveBeenCalledTimes(1);
    const [keyframes, timing] = animate.mock.calls[0];
    expect(keyframes).toEqual([
      { clipPath: 'inset(-50px -624px -118px 0px)' },
      { clipPath: 'inset(50px 0px 50px 0px)' },
    ]);
    expect(timing).toMatchObject({ duration: 220, fill: 'forwards' });

    await settled();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('springs back to the fit after a small pinch-in', async () => {
    await render();
    pinchIn(0.9);
    expectPlacement(FIT);
    await settled();
    expect(onClose).not.toHaveBeenCalled();
    expect(origin.style.visibility).toBe('');
  });

  it('does not close when the touch sequence is cancelled', async () => {
    await render();
    pinchIn(0.4, 'touchcancel');
    expectPlacement(FIT);
    await settled();
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe('MediaViewerModal pinch open', () => {
  // Recognised with the fingers on the centre of the inline image.
  const start = { x: 140, y: 350 };
  const startRect = { left: 40, top: 300, width: 200, height: 100 };

  async function open(scale: number) {
    const gesture = new MediaPinchGesture(start, startRect);
    await render(gesture);
    // Over the inline image to begin with.
    expectPlacement({ scale: 0.25, x: 40, y: 250 });
    gesture.update({ scale, ...start }, 0);
    flushSync();
    return gesture;
  }

  it('stays at the scale under the fingers when released past the fit', async () => {
    const gesture = await open(3);
    const underFingers = { scale: 0.75, x: -160, y: 150 };
    expectPlacement(underFingers);

    gesture.end(false, 1000);
    flushSync();
    expectPlacement(underFingers);
    await settled();
    expectPlacement(underFingers);
    expect(onClose).not.toHaveBeenCalled();
  });

  it('comes back to the scale limit, about the fingers, when released beyond it', async () => {
    const gesture = await open(40);
    expectPlacement({ scale: 10, x: -3860, y: -1700 });

    gesture.end(false, 1000);
    flushSync();
    expectPlacement({ scale: 8, x: -3060, y: -1300 });
  });

  it('closes a gap the fingers left at the edge of the stage', async () => {
    const gesture = await open(3);
    // Dragged right: the picture's left edge is 140px inside the stage.
    gesture.update({ scale: 3, x: 440, y: 350 }, 10);
    flushSync();
    expectPlacement({ scale: 0.75, x: 140, y: 150 });

    gesture.end(false, 1000);
    flushSync();
    expectPlacement({ scale: 0.75, x: 0, y: 150 });
  });

  it('goes on to the fit when released short of it', async () => {
    const gesture = await open(1.5);
    expectPlacement({ scale: 0.375, x: -10, y: 225 });

    gesture.end(false, 1000);
    flushSync();
    expectPlacement(FIT);
    await settled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it('returns to the inline image when let go early', async () => {
    const gesture = await open(1.1);
    gesture.end(false, 1000);
    flushSync();
    expectPlacement({ scale: 0.25, x: 40, y: 250 });
    await settled();
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
