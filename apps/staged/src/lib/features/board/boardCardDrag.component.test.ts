// The card drag gesture against a real DOM: which presses become drags, how
// a drag ends, and that a drag never leaves a click behind to open the card,
// nor eats one that isn't its own. Then what the pointer is over, and the
// scrolling under a card held at the board's side, a frame at a time with
// the frames run by hand.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  createCardDrag,
  createEdgeScroll,
  DRAG_START_DISTANCE,
  dropTargetAcross,
  dropTargetAt,
  dropTargetOf,
  type CardDrag,
  type CardDragHandlers,
  type DragPoint,
} from './boardCardDrag';
import { DRAG_SCROLL_MAX_SPEED } from './boardScroll';

/** A pointer event as a mouse makes them (jsdom has no PointerEvent). */
function pointer(type: string, x: number, y: number, init: MouseEventInit = {}): MouseEvent {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: x,
    clientY: y,
    button: 0,
    buttons: type === 'pointerup' ? 0 : 1,
    ...init,
  });
  Object.defineProperties(event, {
    pointerId: { value: 1, configurable: true },
    pointerType: { value: 'mouse', configurable: true },
    isPrimary: { value: true, configurable: true },
  });
  return event;
}

function withPointerType(event: MouseEvent, pointerType: string): MouseEvent {
  Object.defineProperty(event, 'pointerType', { value: pointerType });
  return event;
}

let card: HTMLButtonElement;
let drag: CardDrag;
let clicked: ReturnType<typeof vi.fn<() => void>>;
let handlers: {
  [K in keyof CardDragHandlers]: ReturnType<typeof vi.fn<CardDragHandlers[K]>>;
};

/** Let the click-swallowing wait run out. */
async function nextTask() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  handlers = {
    begin: vi.fn<CardDragHandlers['begin']>(() => true),
    move: vi.fn<CardDragHandlers['move']>(),
    end: vi.fn<CardDragHandlers['end']>(),
  };
  drag = createCardDrag(handlers);
  clicked = vi.fn<() => void>();
  card = document.createElement('button');
  card.addEventListener('pointerdown', (event) => drag.press('p1', event));
  card.addEventListener('click', clicked);
  document.body.appendChild(card);
});

afterEach(() => {
  drag.cancel();
  document.body.innerHTML = '';
});

describe('createCardDrag', () => {
  it('leaves a press that doesn’t travel as a click', () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 10 + DRAG_START_DISTANCE - 1, 10));
    window.dispatchEvent(pointer('pointerup', 10 + DRAG_START_DISTANCE - 1, 10));
    card.click();

    expect(handlers.begin).not.toHaveBeenCalled();
    expect(handlers.end).not.toHaveBeenCalled();
    expect(clicked).toHaveBeenCalledTimes(1);
  });

  it('begins a drag once the press has travelled, from where it went down', () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 13, 14));

    expect(handlers.begin).toHaveBeenCalledWith('p1', card, { x: 10, y: 10 });
    expect(handlers.move).toHaveBeenLastCalledWith({ x: 13, y: 14 });

    window.dispatchEvent(pointer('pointermove', 200, 40));
    expect(handlers.begin).toHaveBeenCalledTimes(1);
    expect(handlers.move).toHaveBeenLastCalledWith({ x: 200, y: 40 });
  });

  it('drops where the pointer is released, and swallows the click that follows', async () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    window.dispatchEvent(pointer('pointerup', 61, 12));
    // Released over the card it started on, the browser sends a click.
    card.click();

    expect(handlers.end).toHaveBeenCalledWith({ x: 61, y: 12 });
    expect(clicked).not.toHaveBeenCalled();

    // Only that click: the next one is the user's.
    await nextTask();
    card.click();
    expect(clicked).toHaveBeenCalledTimes(1);
  });

  it('stops following the pointer once the card is dropped', () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    window.dispatchEvent(pointer('pointerup', 60, 10));
    handlers.move.mockClear();

    window.dispatchEvent(pointer('pointermove', 90, 10));
    window.dispatchEvent(pointer('pointerup', 90, 10));

    expect(handlers.move).not.toHaveBeenCalled();
    expect(handlers.end).toHaveBeenCalledTimes(1);
  });

  it('calls the drag off on Escape, keeping the key and the release’s click to itself', async () => {
    const pageKeydown = vi.fn();
    document.addEventListener('keydown', pageKeydown);
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));

    const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    document.body.dispatchEvent(escape);

    expect(handlers.end).toHaveBeenCalledWith(null);
    expect(escape.defaultPrevented).toBe(true);
    expect(pageKeydown).not.toHaveBeenCalled();

    // The button is still down: letting go over the card mustn't open it.
    window.dispatchEvent(pointer('pointerup', 60, 10));
    card.click();
    expect(clicked).not.toHaveBeenCalled();
    expect(handlers.end).toHaveBeenCalledTimes(1);

    await nextTask();
    card.click();
    expect(clicked).toHaveBeenCalledTimes(1);
    document.removeEventListener('keydown', pageKeydown);
  });

  it('leaves Escape to the page when no card is in hand', () => {
    const pageKeydown = vi.fn();
    document.addEventListener('keydown', pageKeydown);
    card.dispatchEvent(pointer('pointerdown', 10, 10));

    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

    expect(pageKeydown).toHaveBeenCalledTimes(1);
    document.removeEventListener('keydown', pageKeydown);
  });

  it('calls the drag off when the pointer is lost or the window loses focus', async () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    window.dispatchEvent(pointer('pointercancel', 60, 10));
    expect(handlers.end).toHaveBeenLastCalledWith(null);
    await nextTask();

    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    window.dispatchEvent(new Event('blur'));
    expect(handlers.end).toHaveBeenCalledTimes(2);
    expect(handlers.end).toHaveBeenLastCalledWith(null);

    // The click after a drag called off without a release is the next press's.
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointerup', 10, 10));
    card.click();
    expect(clicked).toHaveBeenCalledTimes(1);
  });

  it('calls the drag off when the button was let go where the page couldn’t see', () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    window.dispatchEvent(pointer('pointermove', 80, 10, { buttons: 0 }));

    expect(handlers.end).toHaveBeenCalledWith(null);
  });

  it('leaves a keyboard click alone after a release it never saw', async () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    // Let go outside the window: no pointerup, and no click, is coming.
    window.dispatchEvent(pointer('pointermove', 80, 10, { buttons: 0 }));
    await nextTask();

    // Enter on the focused card, with no press before it.
    card.click();
    expect(clicked).toHaveBeenCalledTimes(1);
  });

  it('leaves a keyboard click alone while a called-off drag’s button is still down', async () => {
    for (const callOff of [
      () => window.dispatchEvent(new Event('blur')),
      () => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })),
      () => drag.cancel(),
    ]) {
      clicked.mockClear();
      card.dispatchEvent(pointer('pointerdown', 10, 10));
      window.dispatchEvent(pointer('pointermove', 60, 10));
      callOff();

      // The release may be a long time coming, or never seen: a click
      // before it is somebody else's.
      card.click();
      expect(clicked).toHaveBeenCalledTimes(1);
      await nextTask();
      card.click();
      expect(clicked).toHaveBeenCalledTimes(2);

      // The release's own click, when it does come, still opens nothing.
      window.dispatchEvent(pointer('pointerup', 60, 10));
      card.click();
      expect(clicked).toHaveBeenCalledTimes(2);
      await nextTask();
    }
  });

  it('forgets a called-off drag’s click when its pointer is lost', async () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    window.dispatchEvent(new Event('blur'));
    window.dispatchEvent(pointer('pointercancel', 60, 10));

    card.click();
    expect(clicked).toHaveBeenCalledTimes(1);
  });

  it('follows the page scrolling under a held card', () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    handlers.move.mockClear();

    document.body.dispatchEvent(new Event('scroll'));

    expect(handlers.move).toHaveBeenCalledWith({ x: 60, y: 10 });
  });

  it('ignores presses that open a menu or scroll: other buttons, Ctrl-click, touch', () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10, { button: 2, buttons: 2 }));
    window.dispatchEvent(pointer('pointermove', 60, 10, { buttons: 2 }));
    card.dispatchEvent(pointer('pointerdown', 10, 10, { ctrlKey: true }));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    card.dispatchEvent(withPointerType(pointer('pointerdown', 10, 10), 'touch'));
    window.dispatchEvent(pointer('pointermove', 60, 10));

    expect(handlers.begin).not.toHaveBeenCalled();
  });

  it('leaves a refused drag as a click', () => {
    handlers.begin.mockReturnValue(false);
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    window.dispatchEvent(pointer('pointermove', 90, 10));
    window.dispatchEvent(pointer('pointerup', 90, 10));
    card.click();

    expect(handlers.begin).toHaveBeenCalledTimes(1);
    expect(handlers.move).not.toHaveBeenCalled();
    expect(handlers.end).not.toHaveBeenCalled();
    expect(clicked).toHaveBeenCalledTimes(1);
  });

  it('follows one pointer at a time', () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    const other = pointer('pointermove', 90, 10);
    Object.defineProperty(other, 'pointerId', { value: 2 });
    window.dispatchEvent(other);

    expect(handlers.begin).not.toHaveBeenCalled();
  });

  it('ends a drag when cancelled from outside', () => {
    card.dispatchEvent(pointer('pointerdown', 10, 10));
    window.dispatchEvent(pointer('pointermove', 60, 10));
    drag.cancel();
    window.dispatchEvent(pointer('pointermove', 90, 10));

    expect(handlers.end).toHaveBeenCalledWith(null);
    expect(handlers.move).toHaveBeenCalledTimes(1);
  });
});

describe('dropTargetOf', () => {
  function board(): HTMLElement {
    const el = document.createElement('div');
    el.innerHTML = `
      <section data-board-column="done"><button id="card">Card</button></section>
      <section data-board-column="__none__"><p id="empty">No projects</p></section>
      <div data-board-no-drop><button id="add">+</button></div>
      <span id="gap"></span>`;
    document.body.appendChild(el);
    return el;
  }

  it('names the column an element is in', () => {
    const el = board();
    expect(dropTargetOf(el.querySelector('#card'), el)).toEqual({ kind: 'column', key: 'done' });
    expect(dropTargetOf(el.querySelector('#empty'), el)).toEqual({
      kind: 'column',
      key: '__none__',
    });
    expect(dropTargetOf(el.querySelector('[data-board-column="done"]'), el)).toEqual({
      kind: 'column',
      key: 'done',
    });
  });

  it('names the + column, which takes no cards', () => {
    const el = board();
    expect(dropTargetOf(el.querySelector('#add'), el)).toEqual({ kind: 'add-column' });
  });

  it('is nothing between columns, off the board, or over nothing', () => {
    const el = board();
    const outside = document.createElement('section');
    outside.dataset.boardColumn = 'done';
    document.body.appendChild(outside);

    expect(dropTargetOf(el.querySelector('#gap'), el)).toBeNull();
    expect(dropTargetOf(el, el)).toBeNull();
    // Another board's column is not this board's.
    expect(dropTargetOf(outside, el)).toBeNull();
    expect(dropTargetOf(null, el)).toBeNull();
  });
});

describe('dropTargetAcross', () => {
  /** Three 100px-wide zones, 10px apart, from x = 20: done, "No ‹group›", the + column. */
  function board(): HTMLElement {
    const el = document.createElement('div');
    el.innerHTML = `
      <section data-board-column="done"></section>
      <section data-board-column="__none__"></section>
      <div data-board-no-drop></div>`;
    [...el.children].forEach((zone, i) => {
      const left = 20 + i * 110;
      zone.getBoundingClientRect = () =>
        ({ left, right: left + 100, top: 0, bottom: 40 }) as DOMRect;
    });
    document.body.appendChild(el);
    return el;
  }

  it('names the column or + column whose width the pointer is within', () => {
    const el = board();
    expect(dropTargetAcross(20, el)).toEqual({ kind: 'column', key: 'done' });
    expect(dropTargetAcross(119, el)).toEqual({ kind: 'column', key: 'done' });
    expect(dropTargetAcross(180, el)).toEqual({ kind: 'column', key: '__none__' });
    expect(dropTargetAcross(300, el)).toEqual({ kind: 'add-column' });
  });

  it('is nothing before the first column, in a gap, or past the last', () => {
    const el = board();
    expect(dropTargetAcross(19, el)).toBeNull();
    expect(dropTargetAcross(120, el)).toBeNull();
    expect(dropTargetAcross(125, el)).toBeNull();
    expect(dropTargetAcross(340, el)).toBeNull();
  });
});

describe('dropTargetAt', () => {
  let under: Element | null;

  /**
   * A frame holding a board of two 100px columns, done from x = 0 and doing
   * from x = 110, and a chevron laid over the board's side.
   */
  function frame(): { frame: HTMLElement; board: HTMLElement } {
    const el = document.createElement('div');
    el.innerHTML = `
      <div id="board">
        <section data-board-column="done"><button id="card">Card</button></section>
        <section data-board-column="doing"></section>
      </div>
      <button id="chevron">›</button>`;
    const board = el.querySelector<HTMLElement>('#board')!;
    [...board.children].forEach((zone, i) => {
      const left = i * 110;
      zone.getBoundingClientRect = () =>
        ({ left, right: left + 100, top: 0, bottom: 40 }) as DOMRect;
    });
    document.body.appendChild(el);
    return { frame: el, board };
  }

  beforeEach(() => {
    under = null;
    document.elementFromPoint = () => under;
  });

  afterEach(() => {
    delete (document as Partial<Document>).elementFromPoint;
  });

  it('names the column the pointer is on, wherever the pointer is across it', () => {
    const { frame: el, board } = frame();
    under = el.querySelector('#card');
    // The element decides: a card of done, though x is in line with doing.
    expect(dropTargetAt({ x: 150, y: 10 }, board, el)).toEqual({ kind: 'column', key: 'done' });
  });

  it('names the column in line with the pointer under a short column', () => {
    const { frame: el, board } = frame();
    // Below doing's last card there is only the board itself.
    under = board;
    expect(dropTargetAt({ x: 150, y: 300 }, board, el)).toEqual({ kind: 'column', key: 'doing' });
    expect(dropTargetAt({ x: 50, y: 300 }, board, el)).toEqual({ kind: 'column', key: 'done' });
    // The gap between the two is still no column's.
    expect(dropTargetAt({ x: 105, y: 300 }, board, el)).toBeNull();
  });

  it('names the column under what the frame lays over the board', () => {
    const { frame: el, board } = frame();
    under = el.querySelector('#chevron');
    expect(dropTargetAt({ x: 190, y: 20 }, board, el)).toEqual({ kind: 'column', key: 'doing' });
    // Without a frame, only the board's own space counts.
    expect(dropTargetAt({ x: 190, y: 20 }, board)).toBeNull();
  });

  it('is nothing off the board, or where something else covers it', () => {
    const { frame: el, board } = frame();
    // A dialog over the board, in line with a column.
    under = document.body;
    expect(dropTargetAt({ x: 150, y: 20 }, board, el)).toBeNull();
    under = null;
    expect(dropTargetAt({ x: 150, y: 20 }, board, el)).toBeNull();
  });
});

describe('createEdgeScroll', () => {
  let frames: FrameRequestCallback[];
  let point: DragPoint | null;

  /** An 800px-wide view, 400px tall, over 2000px of columns. */
  function scroller(): HTMLElement {
    const el = document.createElement('div');
    let left = 0;
    el.getBoundingClientRect = () => ({ left: 0, right: 800, top: 100, bottom: 500 }) as DOMRect;
    Object.defineProperties(el, {
      scrollWidth: { value: 2000 },
      clientWidth: { value: 800 },
      // As a scroller at 1x does: whole pixels, within its range.
      scrollLeft: {
        get: () => left,
        set: (value: number) => (left = Math.max(0, Math.min(1200, Math.round(value)))),
      },
    });
    return el;
  }

  /** Run the frame that was asked for, if one was. */
  function frame(now: number): boolean {
    const next = frames.shift();
    next?.(now);
    return next !== undefined;
  }

  function edgeScroll(el: HTMLElement) {
    return createEdgeScroll({ scroller: () => el, point: () => point });
  }

  beforeEach(() => {
    frames = [];
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) =>
      frames.push(callback)
    );
    vi.stubGlobal('cancelAnimationFrame', () => (frames = []));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('scrolls toward the side a card is held at, for as long as it is held there', () => {
    const el = scroller();
    const scroll = edgeScroll(el);
    point = { x: 800, y: 300 };

    scroll.follow();
    // The first frame starts the clock; each one after scrolls for its time.
    frame(1000);
    expect(el.scrollLeft).toBe(0);
    frame(1010);
    expect(el.scrollLeft).toBe(DRAG_SCROLL_MAX_SPEED / 100);
    frame(1020);
    expect(el.scrollLeft).toBe(DRAG_SCROLL_MAX_SPEED / 50);

    point = { x: 0, y: 300 };
    frame(1030);
    expect(el.scrollLeft).toBe(DRAG_SCROLL_MAX_SPEED / 100);
  });

  it('keeps going through frames too short to scroll a whole pixel', () => {
    const el = scroller();
    const scroll = edgeScroll(el);
    // Just inside the edge band: a fraction of a pixel per frame.
    point = { x: 737, y: 300 };

    scroll.follow();
    frame(1000);
    for (let now = 1001; now <= 1200; now++) expect(frame(now)).toBe(true);

    expect(el.scrollLeft).toBeGreaterThan(0);
    expect(el.scrollLeft).toBeLessThan(10);
  });

  it('stops at the end of the columns, away from the sides, and once the card is let go', () => {
    const el = scroller();
    const scroll = edgeScroll(el);

    // Nothing further left to show.
    point = { x: 0, y: 300 };
    scroll.follow();
    frame(1000);
    expect(frame(1010)).toBe(false);

    // In the middle of the board.
    point = { x: 400, y: 300 };
    scroll.follow();
    frame(2000);
    expect(frame(2010)).toBe(false);

    // Beside the board's side, but above the columns.
    point = { x: 800, y: 50 };
    scroll.follow();
    frame(3000);
    expect(frame(3010)).toBe(false);

    // Held at the side, then let go.
    point = { x: 800, y: 300 };
    scroll.follow();
    frame(4000);
    frame(4010);
    point = null;
    frame(4020);
    expect(frame(4030)).toBe(false);
    expect(el.scrollLeft).toBe(DRAG_SCROLL_MAX_SPEED / 100);
  });

  it('asks for one frame at a time, and none once stopped', () => {
    const scroll = edgeScroll(scroller());
    point = { x: 800, y: 300 };

    scroll.follow();
    scroll.follow();
    expect(frames).toHaveLength(1);

    scroll.stop();
    expect(frames).toHaveLength(0);
  });

  it('doesn’t throw the board across after a stalled frame', () => {
    const el = scroller();
    const scroll = edgeScroll(el);
    point = { x: 800, y: 300 };

    scroll.follow();
    frame(1000);
    frame(9000);

    expect(el.scrollLeft).toBeLessThanOrEqual(DRAG_SCROLL_MAX_SPEED / 20);
  });
});
