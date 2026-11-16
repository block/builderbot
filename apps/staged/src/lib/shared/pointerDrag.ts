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
 *
 * What the action and every surface share is how far a drag reaches (see
 * `DRAG_REACH`). A pointer that strays a little off the row or list it is
 * dragging along is still dragging along it, and one clearly away is not.
 * The surfaces resolve a point by that rule and the action scrolls by it, so
 * a container never scrolls for a pointer the surface has given up on.
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

/** What of a pointer event says whether it is a press. */
export type PressEvent = Pick<PointerEvent, 'type' | 'button' | 'buttons'>;

/**
 * Whether a pointer event is the primary button going down with no other
 * button held: the press that can arm a drag handle.
 *
 * That is a pointerdown for the primary button, in every engine. It is also
 * a pointermove whose `button` is the primary one and whose `buttons` hold
 * it alone. A pointer sends no second pointerdown while the engine takes a
 * button of its to be down, and reports a button that changes meanwhile on
 * a pointermove. WebKit takes the mouse button to be down for as long as it
 * has seen no release, and a native drag (of a card, a tag row, or text
 * selected in a field) takes the release with it. The next press then
 * arrives as that pointermove and a mousedown, with no pointerdown, and
 * the moves and the release after it arrive as usual.
 *
 * With another button held it is a chord, which arms nothing either way.
 */
export function isPrimaryPress(event: PressEvent): boolean {
  if (event.button !== 0) return false;
  if (event.type === 'pointerdown') return true;
  return event.type === 'pointermove' && event.buttons === 1;
}

/**
 * Whether a pointer event shows a button of the pointer going down: any
 * pointerdown, and a pointermove that reports the primary button pressed
 * (see `isPrimaryPress`), whatever else is held. A handle following a
 * pointer takes one from that pointer as proof that its last release never
 * reached the page.
 */
export function isNewPress(event: PressEvent): boolean {
  if (event.type === 'pointerdown') return true;
  return event.type === 'pointermove' && event.button === 0 && (event.buttons & 1) !== 0;
}

/** How deep each edge's scroll zone runs into the container, in px. */
export const EDGE_SCROLL_ZONE = 48;

/** The scroll speed at an edge and beyond it, in px per second. */
export const EDGE_SCROLL_MAX_SPEED = 900;

/** Where something starts and ends along one axis. */
export interface AxisSpan {
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
export function edgeScrollSpeed(position: number, span: AxisSpan): number {
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

/** The edges of a box in client coordinates, as `getBoundingClientRect` gives them. */
export type DragBox = Pick<DOMRect, 'left' | 'right' | 'top' | 'bottom'>;

/**
 * How far outside the row or list it is dragging along, in px, a pointer
 * still counts as over it. A hand does not hold a straight line: a column
 * dragged sideways by its header drifts above the row, and a group dragged
 * down by its handle drifts off the list's side. Further out than this, the
 * pointer is clearly somewhere else and a release there does nothing.
 */
export const DRAG_REACH = 64;

/**
 * Whether a drag's pointer is over `box` or near enough to count: inside it,
 * or no more than `reach` px past any side.
 */
export function withinDragReach(
  point: PointerDragPoint,
  box: DragBox,
  reach = DRAG_REACH
): boolean {
  return (
    point.clientX >= box.left - reach &&
    point.clientX < box.right + reach &&
    point.clientY >= box.top - reach &&
    point.clientY < box.bottom + reach
  );
}

/**
 * Where along `axis` a drag's pointer counts as being, for a row (`x`) or a
 * list (`y`) that fills `box`. Null once the pointer is out of reach of the
 * box, where the drag lands nowhere.
 *
 * Only the coordinate along the axis counts: one that strays off the other
 * way, within reach, is where it would be had it not strayed. A pointer past
 * either end of the box is at that end, since what runs past the box's edge
 * (it may scroll) does not show.
 */
export function dragPositionAlong(
  point: PointerDragPoint,
  box: DragBox,
  axis: 'x' | 'y',
  reach = DRAG_REACH
): number | null {
  if (!withinDragReach(point, box, reach)) return null;
  const [position, start, end] =
    axis === 'x' ? [point.clientX, box.left, box.right] : [point.clientY, box.top, box.bottom];
  return Math.min(Math.max(position, start), end);
}

/** Where a position along an axis falls among the items of a row or list. */
export interface SpanHit {
  /** The item the position is on. In a gap, the item before the gap. */
  index: number;
  /** In the gap after that item: on neither it nor the next. */
  gap: boolean;
}

/**
 * Where a position along an axis falls among `spans`, the items of a row or
 * list in order along that axis: on the one that holds it, on the first for
 * a position before them all, on the last for one past them all, and
 * otherwise in the gap after one. Null only with no items.
 */
export function spanAlong(position: number, spans: readonly AxisSpan[]): SpanHit | null {
  if (spans.length === 0) return null;
  const final = spans.length - 1;
  if (position < spans[0].start) return { index: 0, gap: false };
  if (position >= spans[final].end) return { index: final, gap: false };
  // The last item to start at or before the position holds it, or is the
  // one the position's gap comes after.
  let index = 0;
  while (index < final && spans[index + 1].start <= position) index++;
  return { index, gap: position >= spans[index].end };
}

/** What the chip that follows a drag's pointer shows. */
export interface PointerDragImage {
  /** The name of what is dragged. */
  label: string;
  /** An icon to lead the name with. The chip takes a copy of it. */
  icon?: Element | null;
}

/** The class of the chip that follows a drag's pointer. Its look is in app.css. */
export const DRAG_CHIP_CLASS = 'pointer-drag-chip';

/** How far the chip's right edge sits left of the pointer, and its top below, in px. */
export const DRAG_CHIP_OFFSET = { x: 8, y: 16 };

/** The least room, in px, the chip keeps from the window's edges. */
export const DRAG_CHIP_MARGIN = 4;

/**
 * Where the chip's top left corner goes, for a pointer at `point`, a chip of
 * `chip`'s size and a window of `viewport`'s.
 *
 * The chip hangs below the pointer and to its left. Below, so it is clear of
 * the pointer's own spot, of the name the pointer is over, and of the names
 * level with it that a column's header passes. To the left, since a group's
 * handle is at its section's left edge: the names and the drop line run to
 * the right from there, and the chip is over neither.
 *
 * At the window's edge the chip slides to stay whole, rather than being cut
 * off. The left and top edges win in a window too small for it.
 */
export function dragChipOrigin(
  point: PointerDragPoint,
  chip: { width: number; height: number },
  viewport: { width: number; height: number }
): { left: number; top: number } {
  const left = point.clientX - DRAG_CHIP_OFFSET.x - chip.width;
  const top = point.clientY + DRAG_CHIP_OFFSET.y;
  return {
    left: Math.max(
      DRAG_CHIP_MARGIN,
      Math.min(left, viewport.width - DRAG_CHIP_MARGIN - chip.width)
    ),
    top: Math.max(
      DRAG_CHIP_MARGIN,
      Math.min(top, viewport.height - DRAG_CHIP_MARGIN - chip.height)
    ),
  };
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
   * move, and again when something scrolls under a still pointer: the
   * drag's own scrolling of its container, or a wheel or trackpad scroll of
   * that container or of anything the handle is inside.
   */
  move(point: PointerDragPoint): boolean;
  /**
   * The pointer was released at `point`. Every release is a drop, whatever
   * `move` last returned: the surface refuses one by doing nothing.
   */
  drop(point: PointerDragPoint): void;
  /** The drag ended without a release: nothing is to be written. */
  cancel(): void;
  /**
   * Scroll this container while the pointer is near its edge. It scrolls
   * only for a pointer within `DRAG_REACH` of it: the surface resolves its
   * points by the same reach (see `dragPositionAlong`), so nothing scrolls
   * where the surface says the drag lands nowhere.
   */
  scroll?: PointerDragScroll;
  /**
   * What the chip that follows the pointer shows, asked once as the drag
   * starts and given the handle. Without it, or with null, nothing follows
   * the pointer.
   */
  image?(handle: HTMLElement): PointerDragImage | null;
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
 * it (see `isPrimaryPress` for what a press is), and `DRAG_THRESHOLD` px of
 * movement starts the drag, so a plain click still clicks. Touch is ignored:
 * a finger scrolls, and the menus are the way to reorder there.
 *
 * A drag ends in `drop` on release, or in `cancel`: on Escape, a
 * pointercancel, the handle losing the pointer's capture, the window losing
 * focus, a release the page never saw, the handle being turned off, and the
 * handle unmounting. One of the two always follows `start`, so the surface
 * needs no guard against a drag that never ended.
 *
 * The click that a release sends after a drag is swallowed, so a handle that
 * is also a button does not act on being dragged. A release the page never
 * saw swallows nothing: the pointer's next press, wherever it is made, ends
 * the wait for it, and that press's click is its own.
 *
 * Given an `image`, a small chip naming what is dragged follows the pointer
 * (see `dragChipOrigin` for where). It is a child of the body, fixed and out
 * of the page's flow, takes no pointer events, and goes when the drag ends,
 * by a drop or by any cancel.
 *
 * The press that arms the handle has no default action of its own. Left to
 * the engine, a mouse press starts a text selection that the engine extends
 * for as long as the button is down, whatever `user-select` says once the
 * drag is under way. WebKit puts that selection, and the keyboard focus
 * with it, into a text field the pointer passes, and leaves the text
 * between the press and the release selected when a drag is cancelled with
 * the button still down. So the armed press's mousedown is cancelled, and
 * nothing the pointer passes selects or takes focus, from the press to the
 * release.
 *
 * Cancelling the mousedown also cancels what a press does to focus, which
 * the action then does by hand (see `movePressFocus`): focus leaves a field
 * being edited, as on any press outside it. From there it stays put, so
 * after a drop it is where the press left it, not where the pointer went.
 *
 * The handle itself needs `user-select: none`, for an engine that selects
 * on a press whose default was not the one cancelled. The action refuses a
 * native drag that starts in the handle (of an image, a link or selected
 * text), which would take the pointer away.
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
  /**
   * Where the drag's own scroll step last left its container. `move` was
   * re-issued there as it stepped, so the scroll event that step sends is
   * not news.
   */
  let stepped: { container: HTMLElement; left: number; top: number } | null = null;
  /** The edges whose zone the pointer has been outside of during this drag. */
  let startEdgeLive = false;
  let endEdgeLive = false;
  let clickGuardTimer: ReturnType<typeof setTimeout> | null = null;
  let chip: HTMLElement | null = null;
  let chipSize = { width: 0, height: 0 };

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

  // ── The chip ──

  /**
   * Put the chip up for the drag that just started, if the surface has an
   * image for it. What must hold wherever the chip is styled from is set
   * here: fixed, so it takes no room in the page, and deaf to the pointer,
   * so what is under it still gets every event.
   */
  function showChip() {
    const image = options.image?.(node);
    if (!image) return;
    const doc = node.ownerDocument;
    chip = doc.createElement('div');
    chip.className = DRAG_CHIP_CLASS;
    chip.setAttribute('aria-hidden', 'true');
    chip.style.position = 'fixed';
    chip.style.left = '0';
    chip.style.top = '0';
    chip.style.pointerEvents = 'none';
    if (image.icon) chip.append(image.icon.cloneNode(true));
    const label = doc.createElement('span');
    label.className = `${DRAG_CHIP_CLASS}-label`;
    label.textContent = image.label;
    chip.append(label);
    doc.body.append(chip);
    // Its size does not change during the drag, so it is read once.
    chipSize = { width: chip.offsetWidth, height: chip.offsetHeight };
    placeChip();
  }

  function placeChip() {
    if (!chip) return;
    const view = node.ownerDocument.defaultView;
    const viewport = { width: view?.innerWidth ?? 0, height: view?.innerHeight ?? 0 };
    const { left, top } = dragChipOrigin(last, chipSize, viewport);
    chip.style.transform = `translate3d(${left}px, ${top}px, 0)`;
  }

  function removeChip() {
    chip?.remove();
    chip = null;
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
    const span = along ? { start: box.left, end: box.right } : { start: box.top, end: box.bottom };

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
    // Out of the container's reach the drag has left it: the surface lands
    // it nowhere there, so nothing scrolls. Within reach it is still over
    // the container, past an end or off to the side, and scrolls as if not.
    if (!withinDragReach(last, box)) speed = 0;

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
    if (after !== before) {
      stepped = { container, left: container.scrollLeft, top: container.scrollTop };
      issueMove();
    }
  }

  /**
   * Something scrolled while the drag is held: by the wheel or a trackpad,
   * by the keyboard, or by the step above. A scroll of the drag's container,
   * or of anything the handle is inside (the document included), moves what
   * the surface resolves against under a pointer that did not move, so the
   * place is resolved again. Without this the drop line and the cursor
   * would describe the old place, and the release, resolved at its own
   * point, would land somewhere else.
   *
   * A scroll event does not bubble, so this listens on the window's capture
   * phase, which hears one from any element.
   */
  function handleScroll(event: Event) {
    if (phase !== 'dragging') return;
    const target = event.target as Node | null;
    const container = options.scroll?.container() ?? null;
    if (!target || (target !== container && !target.contains(node))) return;
    // The step's own event, with nothing else having scrolled the container
    // since: `move` has been issued for this offset already. Each record
    // answers for one event, so an offset the wheel returns to later is new.
    const own =
      stepped !== null &&
      target === stepped.container &&
      stepped.container.scrollLeft === stepped.left &&
      stepped.container.scrollTop === stepped.top;
    if (stepped !== null && target === stepped.container) stepped = null;
    if (!own) issueMove();
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
    stepped = null;
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
    window.addEventListener('pointerdown', handleFollowedPointerDown, true);
    window.addEventListener('pointermove', handlePointerMove, true);
    window.addEventListener('pointerup', handlePointerUp, true);
    window.addEventListener('pointercancel', handlePointerCancel, true);
    window.addEventListener('lostpointercapture', handleLostCapture, true);
    window.addEventListener('keydown', handleKeydown, true);
    window.addEventListener('scroll', handleScroll, true);
    window.addEventListener('blur', handleBlur);
  }

  function unlisten() {
    window.removeEventListener('pointerdown', handleFollowedPointerDown, true);
    window.removeEventListener('pointermove', handlePointerMove, true);
    window.removeEventListener('pointerup', handlePointerUp, true);
    window.removeEventListener('pointercancel', handlePointerCancel, true);
    window.removeEventListener('lostpointercapture', handleLostCapture, true);
    window.removeEventListener('keydown', handleKeydown, true);
    window.removeEventListener('scroll', handleScroll, true);
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
    removeChip();
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
    showChip();
    issueMove();
    startScrolling();
  }

  /**
   * A press on the handle: a pointerdown, or the pointermove an engine
   * sends in its place when it takes the pointer's button to be down
   * already (see `isPrimaryPress`). Any other pointermove over the handle
   * is not this listener's.
   */
  function handlePress(event: PointerEvent) {
    if (event.type !== 'pointerdown' && !isPrimaryPress(event)) return;
    // A press made while a guard is still up is a new gesture: its click is
    // its own.
    dropClickGuard();
    if (phase !== 'idle') abandon(false);
    if (!enabled()) return;
    if (event.pointerType === 'touch' || !event.isPrimary || !isPrimaryPress(event)) return;
    phase = 'armed';
    pointerId = event.pointerId;
    pressed = pointOf(event);
    last = pressed;
    listen();
  }

  /**
   * What a press does to keyboard focus, done by hand: the armed press's
   * mousedown is cancelled, which cancels this with it. A press on
   * something that takes no focus itself gives focus to the nearest thing
   * around it that can hold it (the dialog the handle is in, say), and
   * otherwise leaves nothing focused. Either way it takes focus out of a
   * field being edited, which is how the field hears that the edit is done.
   *
   * Focus already on something the handle is inside stays there. Nothing
   * scrolls for it: the press was made on what is already showing.
   */
  function movePressFocus() {
    const doc = node.ownerDocument;
    const active = doc.activeElement;
    if (!(active instanceof HTMLElement) || active === doc.body || active.contains(node)) return;
    const holder = node.closest<HTMLElement>('[tabindex]');
    if (holder) holder.focus({ preventScroll: true });
    // Nothing around the handle holds focus, or what does would not take it.
    if (doc.activeElement === active) active.blur();
  }

  /**
   * The mousedown of the press that armed the handle. An engine sends it
   * after the pointer event that armed it, in the same task. It has no
   * default action: no selection starts, so there is none for the engine
   * to extend, or to move focus with, while the button is down.
   *
   * A press that did not arm the handle keeps its default: one made with
   * another button, by touch, or while the handle is off (a name field
   * inside it then selects text as usual).
   */
  function handleMouseDown(event: MouseEvent) {
    if (phase !== 'armed' || event.button !== 0) return;
    event.preventDefault();
    movePressFocus();
  }

  /**
   * The pointer this handle is following was pressed again, anywhere in the
   * window. A pointer shows a button going down only when the page saw its
   * last release, or never will: a pointerdown when none of its buttons
   * was down, and where the engine still takes one to be down, a
   * pointermove that reports the primary button pressed (see `isNewPress`).
   * A second button pressed during a drag is neither. So the release this
   * handle was waiting for never reached the page. It went to another
   * window, say, after this one lost focus, and the click that brought the
   * focus back was passed through to the page with no move before it.
   *
   * The gesture is over, without a drop. Left to wait, the release of this
   * new press would be taken for the old one: after a cancelled drag it
   * would swallow the new press's click, during a drag it would drop, and
   * before one it would let a press made elsewhere drag this handle.
   *
   * It listens on the window's capture phase. A press on the handle itself
   * reaches it before the handle's own listener, which then arms afresh.
   * And it never hears the press that armed the handle: that press is past
   * the window's capture phase by the time the handle starts listening.
   */
  function handleFollowedPointerDown(event: PointerEvent) {
    if (event.pointerId !== pointerId) return;
    abandon(false);
  }

  function handlePointerMove(event: PointerEvent) {
    if (event.pointerId !== pointerId) return;
    // A press sent as a move: the release before it was missed.
    if (isNewPress(event)) {
      abandon(false);
      return;
    }
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
      placeChip();
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

  node.addEventListener('pointerdown', handlePress);
  node.addEventListener('pointermove', handlePress);
  node.addEventListener('mousedown', handleMouseDown);
  node.addEventListener('dragstart', handleNativeDragStart);

  return {
    update(next: PointerDragOptions) {
      options = next;
      // A cancelled drag still waiting for its release stays as it is.
      if (!enabled() && (phase === 'armed' || phase === 'dragging')) abandon(true);
      startScrolling();
    },
    destroy() {
      node.removeEventListener('pointerdown', handlePress);
      node.removeEventListener('pointermove', handlePress);
      node.removeEventListener('mousedown', handleMouseDown);
      node.removeEventListener('dragstart', handleNativeDragStart);
      abandon(false);
      dropClickGuard();
    },
  };
}
