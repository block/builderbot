/**
 * Dragging with pointer events: the `pointerDrag` action for a drag handle,
 * and the pure rules it runs on.
 *
 * The desktop build's native file-drop handler answers every drag itself, so
 * the webview never sees an HTML5 drag as a drop destination. A drag made of
 * pointer events starts no native drag, and works the same in every build.
 *
 * The action knows nothing about what is dragged or where it may land. The
 * surface that owns the handle resolves each point the pointer reaches, from
 * element rects and its own rules, and says whether a drop there would act.
 */

import type { ActionReturn } from 'svelte/action';

/** A point in client coordinates. A pointer event is one. */
export interface PointerDragPoint {
  clientX: number;
  clientY: number;
}

/** How far a press must travel, in px, before it is a drag and not a click. */
export const DRAG_THRESHOLD = 4;

/** Whether the pointer has travelled far enough from its press to be dragging. */
export function pastDragThreshold(
  pressed: PointerDragPoint,
  point: PointerDragPoint,
  threshold = DRAG_THRESHOLD
): boolean {
  return Math.hypot(point.clientX - pressed.clientX, point.clientY - pressed.clientY) >= threshold;
}

/** How deep each edge's scroll zone runs into the container, in px. */
export const EDGE_SCROLL_ZONE = 48;

/** The scroll speed at an edge and beyond it, in px per second. */
export const EDGE_SCROLL_MAX_SPEED = 900;

/** Where a container starts and ends along the axis it scrolls on. */
export interface ScrollSpan {
  start: number;
  end: number;
}

/**
 * How fast to scroll, in px per second (negative is towards the start), for
 * a pointer at `position` along the axis of a container spanning `span`.
 * Zero in the middle. The speed climbs from the inner side of an edge's zone
 * to the full speed at the edge, and stays there for a pointer past the edge.
 * A zone is never deeper than a third of the container, so a short container
 * keeps a middle where nothing scrolls.
 */
export function edgeScrollSpeed(position: number, span: ScrollSpan): number {
  const zone = Math.min(EDGE_SCROLL_ZONE, (span.end - span.start) / 3);
  if (!(zone > 0)) return 0;
  const intoStart = span.start + zone - position;
  if (intoStart > 0) return -EDGE_SCROLL_MAX_SPEED * Math.min(1, intoStart / zone);
  const intoEnd = position - (span.end - zone);
  if (intoEnd > 0) return EDGE_SCROLL_MAX_SPEED * Math.min(1, intoEnd / zone);
  return 0;
}

/** The longest gap between two frames that counts in full, in ms. */
const MAX_FRAME_MS = 100;

/**
 * How many whole px to scroll this frame, and the fraction to carry into the
 * next. Not every engine keeps a fractional scroll position, so a slow speed
 * that adds less than a pixel a frame would never move one without the carry.
 * A long gap between frames (a stalled page) counts as `MAX_FRAME_MS`, so
 * the container does not jump.
 */
export function scrollStep(
  speed: number,
  elapsedMs: number,
  carry: number
): { step: number; carry: number } {
  const exact = carry + (speed * Math.min(Math.max(elapsedMs, 0), MAX_FRAME_MS)) / 1000;
  const step = Math.trunc(exact);
  return { step, carry: exact - step };
}

/** The container a drag scrolls when the pointer nears its edge. */
export interface PointerDragScroll {
  /** The element that scrolls, read each frame. Null scrolls nothing. */
  container(): HTMLElement | null;
  /** The axis it scrolls on. */
  axis: 'x' | 'y';
}

export interface PointerDragOptions {
  /** False turns the handle off: a press does nothing, and a drag under way is cancelled. */
  enabled?: boolean;
  /** The press became a drag. `pressed` is where the press was made. */
  start(pressed: PointerDragPoint): void;
  /**
   * The pointer is at `point`. Returns whether a drop there would act, which
   * the cursor shows. Called once straight after `start`, on each pointer
   * move, and again when the drag scrolls its container under a still
   * pointer.
   */
  move(point: PointerDragPoint): boolean;
  /**
   * The pointer was released at `point`. Every release is a drop, whatever
   * `move` last returned: the surface refuses one by doing nothing.
   */
  drop(point: PointerDragPoint): void;
  /** The drag ended without a release: nothing is to be written. */
  cancel(): void;
  /** Scroll this container while the pointer is near its edge. */
  scroll?: PointerDragScroll;
}

/**
 * The attribute the root element carries while a drag is under way, so one
 * rule in app.css sets the cursor whatever the pointer is over: `active`
 * shows grabbing, `refused` (a drop here would do nothing) not-allowed.
 */
export const POINTER_DRAG_ATTRIBUTE = 'data-pointer-drag';

type Phase =
  /** Nothing pressed. */
  | 'idle'
  /** Pressed, not yet past the threshold: still a click. */
  | 'armed'
  /** Past the threshold: the surface has had `start`. */
  | 'dragging'
  /** Cancelled with the button still down: waiting for the release, to swallow its click. */
  | 'spent';

/**
 * The drag-handle action. A primary-button press with a mouse or pen arms
 * it, and `DRAG_THRESHOLD` px of movement starts the drag, so a plain click
 * still clicks. Touch is ignored: a finger scrolls, and the menus are the
 * way to reorder there.
 *
 * A drag ends in `drop` on release, or in `cancel`: on Escape, a
 * pointercancel, the handle losing the pointer's capture, the window losing
 * focus, a release the page never saw, the handle being turned off, and the
 * handle unmounting. One of the two always follows `start`, so the surface
 * needs no guard against a drag that never ended.
 *
 * The click that a release sends after a drag is swallowed, so a handle that
 * is also a button does not act on being dragged.
 *
 * The handle itself needs `user-select: none`, so the press does not start a
 * text selection before it is a drag. The action refuses a native drag that
 * starts in the handle (of an image, a link or selected text), which would
 * take the pointer away.
 */
export function pointerDrag(
  node: HTMLElement,
  initial: PointerDragOptions
): ActionReturn<PointerDragOptions> {
  let options = initial;
  let phase: Phase = 'idle';
  let pointerId = -1;
  let pressed: PointerDragPoint = { clientX: 0, clientY: 0 };
  let last: PointerDragPoint = pressed;
  let captured = false;
  let frame: number | null = null;
  let frameTime: number | null = null;
  let carry = 0;
  /** The edges whose zone the pointer has been outside of during this drag. */
  let startEdgeLive = false;
  let endEdgeLive = false;
  let clickGuardTimer: ReturnType<typeof setTimeout> | null = null;

  const root = node.ownerDocument.documentElement;
  const enabled = () => options.enabled !== false;
  const pointOf = (event: PointerEvent): PointerDragPoint => ({
    clientX: event.clientX,
    clientY: event.clientY,
  });

  function issueMove() {
    const accepted = options.move(last);
    // A callback can end the drag (by turning the handle off, say).
    if (phase === 'dragging') {
      root.setAttribute(POINTER_DRAG_ATTRIBUTE, accepted ? 'active' : 'refused');
    }
  }

  // ── Auto-scroll ──

  function scrollFrame(time: number) {
    const scroll = options.scroll;
    if (!scroll) {
      stopScrolling();
      return;
    }
    // Ask for the next frame first: a `move` below can end the drag, and
    // ending it cancels the frame asked for here.
    frame = requestAnimationFrame(scrollFrame);
    const elapsed = frameTime === null ? 0 : time - frameTime;
    frameTime = time;
    const container = scroll.container();
    if (!container) return;

    const box = container.getBoundingClientRect();
    const along = scroll.axis === 'x';
    const position = along ? last.clientX : last.clientY;
    const across = along ? last.clientY : last.clientX;
    const span = along ? { start: box.left, end: box.right } : { start: box.top, end: box.bottom };
    const inBand = along
      ? across >= box.top && across < box.bottom
      : across >= box.left && across < box.right;

    let speed = edgeScrollSpeed(position, span);
    // A press made inside an edge's zone (on a column half under the edge,
    // say) must not scroll the moment it becomes a drag. That edge scrolls
    // once the pointer has left its zone and come back, or has moved nearer
    // the edge than the press was.
    if (speed >= 0) startEdgeLive = true;
    if (speed <= 0) endEdgeLive = true;
    const pressedAt = along ? pressed.clientX : pressed.clientY;
    if (speed < 0 && !startEdgeLive && position >= pressedAt) speed = 0;
    if (speed > 0 && !endEdgeLive && position <= pressedAt) speed = 0;
    // Beside the container, not over it, the drag has left it: nothing there
    // takes a drop, so nothing scrolls.
    if (!inBand) speed = 0;

    if (speed === 0) {
      carry = 0;
      return;
    }
    const next = scrollStep(speed, elapsed, carry);
    carry = next.carry;
    if (next.step === 0) return;

    const before = along ? container.scrollLeft : container.scrollTop;
    if (along) container.scrollLeft = before + next.step;
    else container.scrollTop = before + next.step;
    const after = along ? container.scrollLeft : container.scrollTop;
    // What is under the pointer changed without the pointer moving.
    if (after !== before) issueMove();
  }

  function startScrolling() {
    if (phase === 'dragging' && options.scroll && frame === null) {
      frame = requestAnimationFrame(scrollFrame);
    }
  }

  function stopScrolling() {
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
    frameTime = null;
    carry = 0;
  }

  // ── Click suppression ──

  function swallowClick(event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();
    dropClickGuard();
  }

  function dropClickGuard() {
    window.removeEventListener('click', swallowClick, true);
    if (clickGuardTimer !== null) clearTimeout(clickGuardTimer);
    clickGuardTimer = null;
  }

  /**
   * Swallow the click this release sends, if it sends one. The click comes
   * in the same task as the release, so a timer bounds the guard: a release
   * that sends no click (it landed off the element the press was on) must
   * not cost the next real click.
   */
  function guardNextClick() {
    dropClickGuard();
    window.addEventListener('click', swallowClick, true);
    clickGuardTimer = setTimeout(dropClickGuard, 0);
  }

  // ── The drag ──

  function listen() {
    window.addEventListener('pointermove', handlePointerMove, true);
    window.addEventListener('pointerup', handlePointerUp, true);
    window.addEventListener('pointercancel', handlePointerCancel, true);
    window.addEventListener('lostpointercapture', handleLostCapture, true);
    window.addEventListener('keydown', handleKeydown, true);
    window.addEventListener('blur', handleBlur);
  }

  function unlisten() {
    window.removeEventListener('pointermove', handlePointerMove, true);
    window.removeEventListener('pointerup', handlePointerUp, true);
    window.removeEventListener('pointercancel', handlePointerCancel, true);
    window.removeEventListener('lostpointercapture', handleLostCapture, true);
    window.removeEventListener('keydown', handleKeydown, true);
    window.removeEventListener('blur', handleBlur);
  }

  function releaseCapture() {
    if (!captured) return;
    captured = false;
    try {
      node.releasePointerCapture?.(pointerId);
    } catch {
      // The pointer is already gone.
    }
  }

  /** Back to nothing pressed, or to `spent` while a cancelled drag's button is still down. */
  function settle(next: 'idle' | 'spent') {
    // Only this handle's own mark: another handle's drag may be under way.
    if (phase === 'dragging') root.removeAttribute(POINTER_DRAG_ATTRIBUTE);
    phase = next;
    stopScrolling();
    releaseCapture();
    if (next === 'idle') unlisten();
  }

  /**
   * End the press without a drop. The surface hears `cancel` only if it had
   * `start`. With `awaitRelease`, the button is taken to be still down, and
   * the release to come is watched for so its click can be swallowed.
   */
  function abandon(awaitRelease: boolean) {
    const wasDragging = phase === 'dragging';
    // Tidy up first, so a callback that throws leaves nothing behind.
    settle(wasDragging && awaitRelease ? 'spent' : 'idle');
    if (wasDragging) options.cancel();
  }

  function beginDrag(event: PointerEvent) {
    phase = 'dragging';
    last = pointOf(event);
    startEdgeLive = false;
    endEdgeLive = false;
    // Capture from here, not from the press: a captured pointer's click goes
    // to the handle, which would take a plain click from a button inside it.
    // jsdom has no pointer capture, and a pointer that has gone throws.
    try {
      node.setPointerCapture?.(pointerId);
      captured = typeof node.setPointerCapture === 'function';
    } catch {
      captured = false;
    }
    root.setAttribute(POINTER_DRAG_ATTRIBUTE, 'active');
    options.start(pressed);
    if (phase !== 'dragging') return;
    issueMove();
    startScrolling();
  }

  function handlePointerDown(event: PointerEvent) {
    // A press made while a guard is still up is a new gesture: its click is
    // its own.
    dropClickGuard();
    if (phase !== 'idle') abandon(false);
    if (!enabled()) return;
    if (event.pointerType === 'touch' || !event.isPrimary || event.button !== 0) return;
    phase = 'armed';
    pointerId = event.pointerId;
    pressed = pointOf(event);
    last = pressed;
    listen();
  }

  function handlePointerMove(event: PointerEvent) {
    if (event.pointerId !== pointerId) return;
    // The button is up, so the release went somewhere the page never saw
    // (outside the window with no capture, or under a context menu).
    if ((event.buttons & 1) === 0) {
      abandon(false);
      return;
    }
    if (phase === 'armed') {
      if (pastDragThreshold(pressed, event)) beginDrag(event);
    } else if (phase === 'dragging') {
      last = pointOf(event);
      issueMove();
    }
  }

  function handlePointerUp(event: PointerEvent) {
    if (event.pointerId !== pointerId) return;
    const wasDragging = phase === 'dragging';
    const swallow = wasDragging || phase === 'spent';
    settle('idle');
    if (swallow) guardNextClick();
    if (wasDragging) options.drop(pointOf(event));
  }

  function handlePointerCancel(event: PointerEvent) {
    if (event.pointerId !== pointerId) return;
    // The pointer is gone: no release and no click will follow.
    abandon(false);
  }

  function handleLostCapture(event: PointerEvent) {
    // A release gives up the capture too, but by then the drag has dropped.
    if (event.pointerId !== pointerId || event.target !== node) return;
    captured = false;
    if (phase === 'dragging') abandon(true);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (phase !== 'dragging' || event.key !== 'Escape') return;
    // Escape ends the drag and nothing else: not the dialog the handle is in.
    event.preventDefault();
    event.stopImmediatePropagation();
    abandon(true);
  }

  /**
   * The window lost focus. An element's blur does not bubble, and this
   * listener is not a capturing one, so it hears only the window's own.
   */
  function handleBlur() {
    if (phase === 'dragging') abandon(true);
  }

  function handleNativeDragStart(event: DragEvent) {
    if (enabled()) event.preventDefault();
  }

  node.addEventListener('pointerdown', handlePointerDown);
  node.addEventListener('dragstart', handleNativeDragStart);

  return {
    update(next: PointerDragOptions) {
      options = next;
      // A cancelled drag still waiting for its release stays as it is.
      if (!enabled() && (phase === 'armed' || phase === 'dragging')) abandon(true);
      startScrolling();
    },
    destroy() {
      node.removeEventListener('pointerdown', handlePointerDown);
      node.removeEventListener('dragstart', handleNativeDragStart);
      abandon(false);
      dropClickGuard();
    },
  };
}
