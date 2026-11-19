import type { Attachment } from 'svelte/attachments';
import type { DiagramZoomTransform } from './diagramZoom';
import { getMarkdownMedia, type MarkdownMedia } from './mediaViewer';

/**
 * Pinch-out on an inline note image opens it in the full-screen viewer, with
 * the image following the fingers on the way.
 *
 * The fingers land on the inline image and stay its touches for the whole
 * gesture, so the viewer's own zoom behaviour never sees them. The recogniser
 * here sits on the markdown's container, mounts the viewer once the fingers
 * have clearly spread, and hands it a `MediaPinchGesture` to follow.
 */

export interface PinchFrame {
  /** Finger distance over the distance when the gesture was recognised. */
  scale: number;
  /** Midpoint of the two fingers, in client coordinates. */
  x: number;
  y: number;
}

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface MediaPinchListener {
  frame: (frame: PinchFrame) => void;
  /** The fingers lifted: `open` is whether the viewer should stay open. */
  end: (open: boolean) => void;
}

/** What the viewer needs to know about how it was opened. */
export interface OpenMarkdownMedia extends MarkdownMedia {
  /** The inline image the viewer was opened from, to return to on a pinch close. */
  originImage?: HTMLImageElement | null;
  /** Set when a pinch is opening the viewer and is still in progress. */
  gesture?: MediaPinchGesture | null;
}

// Fingers have to spread this far before the pinch is taken as an open.
const RECOGNISE_SPREAD_PX = 10;
// Release at or past this scale opens the viewer; short of it the image goes back.
const OPEN_COMMIT_SCALE = 1.2;
// A quick spread opens from a smaller distance.
const OPEN_FLICK_SCALE = 1.06;
const OPEN_FLICK_SPEED = 1.2;
const FLICK_WINDOW_MS = 120;
/** The viewer's backdrop is fully in at this scale. */
export const OPEN_FULL_SCALE = 1.5;

/** One pinch on one inline image, from recognition until the fingers lift. */
export class MediaPinchGesture {
  /** Midpoint of the fingers when the gesture was recognised. */
  readonly start: { x: number; y: number };
  /** The image's on-screen box at that moment. */
  readonly startRect: Rect;
  latest: PinchFrame;
  /** Null while the fingers are down. */
  outcome: 'open' | 'cancel' | null = null;
  private listeners = new Set<MediaPinchListener>();
  private samples: Array<{ time: number; scale: number }> = [];

  constructor(start: { x: number; y: number }, startRect: Rect) {
    this.start = start;
    this.startRect = startRect;
    this.latest = { scale: 1, x: start.x, y: start.y };
  }

  /** Follow the gesture. A listener added late is caught up with the latest frame, or the end. */
  subscribe(listener: MediaPinchListener): () => void {
    this.listeners.add(listener);
    listener.frame(this.latest);
    if (this.outcome) listener.end(this.outcome === 'open');
    return () => this.listeners.delete(listener);
  }

  update(frame: PinchFrame, time = performance.now()): void {
    if (this.outcome) return;
    this.latest = frame;
    this.samples.push({ time, scale: frame.scale });
    while (this.samples.length > 2 && time - this.samples[1].time >= FLICK_WINDOW_MS) {
      this.samples.shift();
    }
    for (const listener of this.listeners) listener.frame(frame);
  }

  /** The fingers lifted. A cancelled touch sequence never opens. */
  end(cancelled = false, time = performance.now()): void {
    if (this.outcome) return;
    const { scale } = this.latest;
    const open =
      !cancelled &&
      (scale >= OPEN_COMMIT_SCALE || (scale >= OPEN_FLICK_SCALE && this.isSpreadingFast(time)));
    this.outcome = open ? 'open' : 'cancel';
    for (const listener of this.listeners) listener.end(open);
  }

  private isSpreadingFast(time: number): boolean {
    const first = this.samples[0];
    const last = this.samples.at(-1);
    if (!first || !last || last.time <= first.time) return false;
    // A pinch held still before the fingers lift is not a flick.
    if (time - last.time > FLICK_WINDOW_MS) return false;
    return ((last.scale - first.scale) / (last.time - first.time)) * 1000 >= OPEN_FLICK_SPEED;
  }
}

/**
 * The box the picture itself occupies inside an `<img>`. The inline image is
 * `object-fit: contain` under a height cap, so its element box can be wider
 * or taller than the picture.
 */
export function containedImageRect(box: Rect, natural: { width: number; height: number }): Rect {
  if (natural.width <= 0 || natural.height <= 0 || box.width <= 0 || box.height <= 0) return box;
  const scale = Math.min(box.width / natural.width, box.height / natural.height);
  const width = natural.width * scale;
  const height = natural.height * scale;
  return {
    left: box.left + (box.width - width) / 2,
    top: box.top + (box.height - height) / 2,
    width,
    height,
  };
}

export interface ImagePlace {
  /** The whole picture, including any part that is scrolled out of view. */
  rect: Rect;
  /**
   * The area the picture shows through when part of it is cut off by the pane
   * it scrolls in (or the window); null when all of it is in view.
   */
  clip: Rect | null;
}

/**
 * Where an inline image is on screen, or null when there is nowhere to
 * return to: it is gone, not laid out, or scrolled out of view.
 */
export function visibleImagePlace(image: HTMLImageElement | null | undefined): ImagePlace | null {
  if (!image?.isConnected) return null;
  const box = image.getBoundingClientRect();
  if (box.width <= 0 || box.height <= 0) return null;
  const view = image.ownerDocument.defaultView;
  if (!view) return null;
  // Clipped by the pane it scrolls in counts as out of view as well.
  let left = 0;
  let top = 0;
  let right = view.innerWidth;
  let bottom = view.innerHeight;
  for (let el = image.parentElement; el; el = el.parentElement) {
    const { overflowX, overflowY } = view.getComputedStyle(el);
    if (overflowX === 'visible' && overflowY === 'visible') continue;
    const clip = el.getBoundingClientRect();
    left = Math.max(left, clip.left);
    top = Math.max(top, clip.top);
    right = Math.min(right, clip.right);
    bottom = Math.min(bottom, clip.bottom);
  }
  const rect = containedImageRect(box, { width: image.naturalWidth, height: image.naturalHeight });
  const rectRight = rect.left + rect.width;
  const rectBottom = rect.top + rect.height;
  if (rectRight <= left || rect.left >= right || rectBottom <= top || rect.top >= bottom) {
    return null;
  }
  const whole = rect.left >= left && rect.top >= top && rectRight <= right && rectBottom <= bottom;
  return {
    rect,
    clip: whole ? null : { left, top, width: right - left, height: bottom - top },
  };
}

/** The on-screen box of an inline image's picture, or null when none of it is in view. */
export function visibleImageRect(image: HTMLImageElement | null | undefined): Rect | null {
  return visibleImagePlace(image)?.rect ?? null;
}

/** The viewer transform that lays content of `size` exactly over `rect` on screen. */
export function transformOverRect(
  rect: Rect,
  stage: { left: number; top: number },
  size: { width: number; height: number }
): DiagramZoomTransform {
  return {
    scale: rect.width / size.width,
    offsetX: rect.left - stage.left,
    offsetY: rect.top - stage.top,
  };
}

/**
 * `origin` scaled by the pinch about the point the fingers started on, and
 * carried along with them: what was under the fingers stays under them.
 */
export function pinchTransform(
  origin: DiagramZoomTransform,
  start: { x: number; y: number },
  frame: PinchFrame,
  stage: { left: number; top: number }
): DiagramZoomTransform {
  return {
    scale: origin.scale * frame.scale,
    offsetX: frame.x - stage.left - (start.x - stage.left - origin.offsetX) * frame.scale,
    offsetY: frame.y - stage.top - (start.y - stage.top - origin.offsetY) * frame.scale,
  };
}

/** How far the open has come, for the viewer's backdrop and header. */
export function pinchOpenProgress(scale: number): number {
  return Math.min(1, Math.max(0, (scale - 1) / (OPEN_FULL_SCALE - 1)));
}

function inlineImageForTouch(touch: Touch): HTMLImageElement | null {
  const target = touch.target;
  if (!(target instanceof Element)) return null;
  const image = target.closest('.markdown-media-image')?.querySelector('img');
  return image && image.complete && image.naturalWidth > 0 ? image : null;
}

function touchSpan(a: Touch, b: Touch) {
  return {
    distance: Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY),
    x: (a.clientX + b.clientX) / 2,
    y: (a.clientY + b.clientY) / 2,
  };
}

/**
 * For the element that holds rendered markdown (or any ancestor its touches
 * bubble to). `onOpen` is called once per recognised pinch-out, with the
 * media to show and the gesture for the viewer to follow.
 *
 * Only one of the two fingers has to be on the image: with the other on the
 * text beside it the image's own element would see a single touch, which is
 * why this listens above the figure.
 *
 * Touch events stay with the element a finger landed on. Once two fingers
 * are down the rest of the sequence is therefore followed on those elements
 * as well: one of them may be outside this container, and one may be removed
 * from it mid-gesture (a re-render of the markdown), after which its events
 * no longer bubble here.
 *
 * Preventing the moves and Safari's gesture events is what keeps the browser
 * out of it: mobile Safari otherwise zooms the page on a pinch-out, whatever
 * the viewport meta says, and shows its tab overview on a pinch-in. There is
 * deliberately no `touch-action` rule on the figure. `pan-x pan-y` there
 * stops the page zoom too, but in mobile Safari (iOS 27.2) it also stopped a
 * one-finger drag that starts on the image from scrolling the note.
 */
export function mediaPinchOpen(
  onOpen: (media: OpenMarkdownMedia) => void
): Attachment<HTMLElement> {
  return (node) => {
    // `pending`: two fingers are down on an image and have not spread yet.
    // `drained`: the gesture is over or was refused, and the fingers that are
    // still down do nothing more.
    let state: 'idle' | 'pending' | 'active' | 'drained' = 'idle';
    let image: HTMLImageElement | null = null;
    let baseDistance = 0;
    let gesture: MediaPinchGesture | null = null;
    // The fingers of the sequence and the elements they landed on.
    let fingers: number[] = [];
    let targets: EventTarget[] = [];
    // An event followed on its target is seen again when it bubbles here.
    let handled: Event | null = null;

    const active = { passive: false } as const;

    function finish(cancelled: boolean) {
      gesture?.end(cancelled);
      gesture = null;
      image = null;
    }

    function follow(touches: Touch[]) {
      fingers = touches.map((touch) => touch.identifier);
      targets = [...new Set(touches.map((touch) => touch.target))].filter(
        (target) => target !== node
      );
      for (const target of targets) {
        target.addEventListener('touchmove', handleMove as EventListener, active);
        target.addEventListener('touchend', handleEnd as EventListener);
        target.addEventListener('touchcancel', handleEnd as EventListener);
        target.addEventListener('gesturestart', handleGesture, active);
        target.addEventListener('gesturechange', handleGesture, active);
      }
    }

    function toIdle() {
      for (const target of targets) {
        target.removeEventListener('touchmove', handleMove as EventListener);
        target.removeEventListener('touchend', handleEnd as EventListener);
        target.removeEventListener('touchcancel', handleEnd as EventListener);
        target.removeEventListener('gesturestart', handleGesture);
        target.removeEventListener('gesturechange', handleGesture);
      }
      fingers = [];
      targets = [];
      state = 'idle';
    }

    function handleStart(event: TouchEvent) {
      // The fingers of the last sequence are gone without their end having
      // been seen: start afresh rather than hold the new touch.
      if (
        state !== 'idle' &&
        !Array.from(event.touches).some((touch) => fingers.includes(touch.identifier))
      ) {
        finish(true);
        toIdle();
      }
      if (state === 'idle' && event.touches.length === 2) {
        const [a, b] = [event.touches[0], event.touches[1]];
        image = inlineImageForTouch(a) ?? inlineImageForTouch(b);
        // Not cancelable means the first finger is already scrolling the pane.
        if (!image || !event.cancelable) {
          image = null;
          return;
        }
        baseDistance = touchSpan(a, b).distance;
        if (baseDistance <= 0) return;
        state = 'pending';
        follow([a, b]);
      } else if (state !== 'idle' && event.touches.length > 2) {
        finish(true);
        state = 'drained';
      }
    }

    function handleMove(event: TouchEvent) {
      if (state === 'idle' || event === handled) return;
      handled = event;
      if (event.cancelable) event.preventDefault();
      if (state === 'drained' || event.touches.length !== 2) return;
      const span = touchSpan(event.touches[0], event.touches[1]);
      if (state === 'pending') {
        if (span.distance - baseDistance < RECOGNISE_SPREAD_PX) return;
        const rect = visibleImageRect(image);
        const media = image && getMarkdownMedia(image);
        if (!rect || !media) {
          state = 'drained';
          return;
        }
        // Measured from here on, so the image starts out exactly in place.
        baseDistance = span.distance;
        gesture = new MediaPinchGesture({ x: span.x, y: span.y }, rect);
        state = 'active';
        onOpen({ ...media, originImage: image, gesture });
        return;
      }
      gesture?.update({ scale: span.distance / baseDistance, x: span.x, y: span.y });
    }

    function handleEnd(event: TouchEvent) {
      if (state === 'idle' || event === handled) return;
      handled = event;
      if (event.touches.length < 2 && state !== 'drained') {
        finish(event.type === 'touchcancel');
        state = 'drained';
      }
      if (event.touches.length === 0) toIdle();
    }

    // Safari's own pinch recogniser, which the viewport meta does not stop.
    function handleGesture(event: Event) {
      if (state !== 'idle' && event.cancelable) event.preventDefault();
    }

    node.addEventListener('touchstart', handleStart, { passive: true });
    node.addEventListener('touchmove', handleMove, active);
    node.addEventListener('touchend', handleEnd);
    node.addEventListener('touchcancel', handleEnd);
    node.addEventListener('gesturestart', handleGesture, active);
    node.addEventListener('gesturechange', handleGesture, active);
    return () => {
      node.removeEventListener('touchstart', handleStart);
      node.removeEventListener('touchmove', handleMove);
      node.removeEventListener('touchend', handleEnd);
      node.removeEventListener('touchcancel', handleEnd);
      node.removeEventListener('gesturestart', handleGesture);
      node.removeEventListener('gesturechange', handleGesture);
      finish(true);
      toIdle();
    };
  };
}
