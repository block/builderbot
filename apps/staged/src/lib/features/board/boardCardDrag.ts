/**
 * The gesture that drags a board card: a press on the card that travels far
 * enough becomes a drag, tracked on the window until the pointer is released
 * (a drop), or until Escape, a lost pointer or the window losing focus calls
 * it off.
 *
 * Built on pointer events, not HTML5 drag and drop. The desktop app's webview
 * takes native drags for file drops (see branches/dragDrop.ts), and with that
 * on the page never hears `dragover` or `drop` for a drag of its own: a card
 * could be picked up but not put down.
 *
 * A press that never travels stays a click, and the click a drag would
 * otherwise end in (a card released over itself) is swallowed, so dragging a
 * card never opens it.
 *
 * Also here: reading what the pointer is over off the board's markup, and
 * scrolling the columns under a card held at the board's side, which a
 * native drag would have done for itself.
 */

import { edgeScrollVelocity, overflowEdges } from './boardScroll';

/** How far a press travels, in px, before it is a drag. */
export const DRAG_START_DISTANCE = 5;

export interface DragPoint {
  x: number;
  y: number;
}

export interface CardDragHandlers {
  /**
   * A press on `card` travelled far enough to be a drag; `press` is where it
   * went down. False refuses the drag, and the press stays a click.
   */
  begin(projectId: string, card: HTMLElement, press: DragPoint): boolean;
  /** The pointer moved, or the page scrolled under it. */
  move(point: DragPoint): void;
  /** The card was released at `point`, or the drag was called off (null). */
  end(point: DragPoint | null): void;
}

export interface CardDrag {
  /** A card's pointerdown. */
  press(projectId: string, event: PointerEvent): void;
  /** Call off a drag in progress, or forget a press that hasn't become one. */
  cancel(): void;
}

/** What a dragged card is over: a column, or the trailing + column. */
export type BoardDropTarget = { kind: 'column'; key: string } | { kind: 'add-column' };

/**
 * The drop target `element` belongs to, by the board's own markers: a
 * column's `data-board-column`, or `data-board-no-drop` on what takes no
 * cards (the + column). Null outside `board`.
 */
export function dropTargetOf(element: Element | null, board: Element): BoardDropTarget | null {
  if (!element || !board.contains(element)) return null;
  const zone = element.closest<HTMLElement>('[data-board-column], [data-board-no-drop]');
  if (!zone || !board.contains(zone)) return null;
  const key = zone.dataset.boardColumn;
  return key === undefined ? { kind: 'add-column' } : { kind: 'column', key };
}

function pointOf(event: PointerEvent): DragPoint {
  return { x: event.clientX, y: event.clientY };
}

/**
 * Swallow the click the press in progress ends in. `released` says the
 * pointer is already up, so the click, if there is one, comes within this
 * task; otherwise the wait runs until the press ends, or a new one starts.
 */
function swallowPressClick(released: boolean): void {
  const swallow = (event: Event) => {
    event.preventDefault();
    event.stopPropagation();
    done();
  };
  const done = () => {
    window.removeEventListener('click', swallow, true);
    window.removeEventListener('pointerup', soon, true);
    window.removeEventListener('pointercancel', soon, true);
    window.removeEventListener('pointerdown', done, true);
  };
  // A click follows its pointerup within the task.
  const soon = () => setTimeout(done, 0);
  window.addEventListener('click', swallow, true);
  if (released) {
    soon();
  } else {
    window.addEventListener('pointerup', soon, true);
    window.addEventListener('pointercancel', soon, true);
    window.addEventListener('pointerdown', done, true);
  }
}

export function createCardDrag(handlers: CardDragHandlers): CardDrag {
  let gesture: {
    pointerId: number;
    projectId: string;
    card: HTMLElement;
    press: DragPoint;
    last: DragPoint;
    dragging: boolean;
  } | null = null;

  function onMove(event: PointerEvent) {
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    // The release happened where the page couldn't see it.
    if (event.buttons === 0) return finish(null, false);
    const point = pointOf(event);
    gesture.last = point;
    if (!gesture.dragging) {
      const travelled = Math.hypot(point.x - gesture.press.x, point.y - gesture.press.y);
      if (travelled < DRAG_START_DISTANCE) return;
      if (!handlers.begin(gesture.projectId, gesture.card, gesture.press)) return stop();
      gesture.dragging = true;
    }
    handlers.move(point);
  }

  function onUp(event: PointerEvent) {
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    finish(pointOf(event), true);
  }

  function onCancel(event: PointerEvent) {
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    finish(null, true);
  }

  function onKeydown(event: KeyboardEvent) {
    if (!gesture?.dragging || event.key !== 'Escape') return;
    // The drag's Escape, not the page's: it doesn't also clear a search.
    event.preventDefault();
    event.stopPropagation();
    finish(null, false);
  }

  function onScroll() {
    if (gesture?.dragging) handlers.move(gesture.last);
  }

  function onBlur() {
    finish(null, false);
  }

  // Window-wide, capturing, so the gesture follows the pointer off the board
  // and past handlers that stop propagation. Blur doesn't bubble, so a plain
  // listener hears only the window's own.
  function listen() {
    window.addEventListener('pointermove', onMove, true);
    window.addEventListener('pointerup', onUp, true);
    window.addEventListener('pointercancel', onCancel, true);
    window.addEventListener('keydown', onKeydown, true);
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('blur', onBlur);
  }

  function stop() {
    gesture = null;
    window.removeEventListener('pointermove', onMove, true);
    window.removeEventListener('pointerup', onUp, true);
    window.removeEventListener('pointercancel', onCancel, true);
    window.removeEventListener('keydown', onKeydown, true);
    window.removeEventListener('scroll', onScroll, true);
    window.removeEventListener('blur', onBlur);
  }

  /** End the gesture: a drop at `point`, or called off with null. */
  function finish(point: DragPoint | null, released: boolean) {
    if (!gesture) return;
    const wasDragging = gesture.dragging;
    stop();
    if (!wasDragging) return;
    swallowPressClick(released);
    handlers.end(point);
  }

  return {
    press(projectId, event) {
      if (gesture) return;
      // The primary button of a mouse or pen. A touch scrolls, the secondary
      // button and Ctrl-click open the context menu.
      if (event.button !== 0 || event.ctrlKey || event.isPrimary === false) return;
      if (event.pointerType === 'touch') return;
      const press = pointOf(event);
      gesture = {
        pointerId: event.pointerId,
        projectId,
        card: event.currentTarget as HTMLElement,
        press,
        last: press,
        dragging: false,
      };
      listen();
    },
    cancel() {
      finish(null, false);
      stop();
    },
  };
}

export interface EdgeScroll {
  /** The card moved: start scrolling if it is now held at a side. */
  follow(): void;
  stop(): void;
}

/** The longest stretch, in ms, one frame scrolls for: a stalled tab doesn't throw the board across. */
const MAX_FRAME_MS = 50;

/**
 * Scroll `scroller` sideways while a dragged card is held near one of its
 * sides (edgeScrollVelocity), a frame at a time for as long as it stays there
 * and there is more to show. `point` is where the card is, null once it is
 * let go; `scrolled` is told after each step, since what is under a still
 * pointer has changed.
 *
 * It scrolls whole pixels and carries the rest over: a scroller rounds a
 * fraction of a pixel away, which would read as having reached the end.
 */
export function createEdgeScroll(options: {
  scroller: () => HTMLElement | null;
  point: () => DragPoint | null;
  scrolled: () => void;
}): EdgeScroll {
  let frame = 0;
  let frameAt: number | null = null;
  let owed = 0;

  function step(now: number) {
    frame = 0;
    const el = options.scroller();
    const point = options.point();
    if (!el || !point) return;
    const box = el.getBoundingClientRect();
    const beside = point.y >= box.top && point.y <= box.bottom;
    const velocity = beside ? edgeScrollVelocity(point.x, box) : 0;
    const more = overflowEdges(el);
    if (velocity === 0 || !(velocity < 0 ? more.start : more.end)) return;
    // The first frame only starts the clock.
    const elapsed = frameAt === null ? 0 : Math.min(Math.max(now - frameAt, 0), MAX_FRAME_MS);
    frameAt = now;
    owed += (velocity * elapsed) / 1000;
    const whole = Math.trunc(owed);
    if (whole !== 0) {
      owed -= whole;
      el.scrollLeft += whole;
      options.scrolled();
    }
    frame = requestAnimationFrame(step);
  }

  return {
    follow() {
      if (frame !== 0) return;
      frameAt = null;
      owed = 0;
      frame = requestAnimationFrame(step);
    },
    stop() {
      cancelAnimationFrame(frame);
      frame = 0;
    },
  };
}
