import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import TimelineWithMenu, { type TimelineWithMenuRow } from './testStubs/TimelineWithMenu.svelte';
import {
  LONG_PRESS_DELAY_MS,
  SCROLL_STILL_TOLERANCE_PX,
} from '$lib/components/ui/context-menu/longPress';
import { reactiveProps } from '$lib/features/sessions/testStubs/reactiveProps.svelte';

// One trigger wraps the whole timeline, so a touch long press has to end at
// the row it started on. The hard case is a row whose touched element is
// replaced during the hold, which a live title re-rendered through `{@html}`
// does: the press then fires at the trigger, with no row to be found from it,
// and a scroll in the meantime has to be judged by where that row is.

const ROW_HEIGHT = 44;
const LIST_LEFT = 16;
const LIST_WIDTH = 320;
const BITS_UI_LONG_PRESS_MS = 700;

let component: ReturnType<typeof mount> | null = null;
/** Stands in for the pane the timeline scrolls in. */
let pane: HTMLElement;
let onDelete: ReturnType<typeof vi.fn<(key: string) => void>>;
/** Where the top of the list is on screen: a scroll of the pane moves it. */
let listTop: number;

function title(text: string) {
  return `<code>${text}</code>`;
}

function render(rows: TimelineWithMenuRow[]) {
  const props = reactiveProps({ rows, onDelete });
  component = mount(TimelineWithMenu, { target: pane, props });
  flushSync();
  return props;
}

function rowFor(key: string): HTMLElement {
  const row = pane.querySelector<HTMLElement>(`[data-timeline-context-menu-key="${key}"]`);
  if (!row) throw new Error(`row ${key} not rendered`);
  return row;
}

function rowElements(): Element[] {
  return Array.from(pane.querySelectorAll('.timeline-row'));
}

/** The element a finger on a row's title lands on. */
function titleCode(row: Element): HTMLElement {
  const code = row.querySelector<HTMLElement>('.timeline-title code');
  if (!code) throw new Error('title not rendered');
  return code;
}

/** A point inside the row at `index`, in the list as it is now. */
function pointInRow(index: number) {
  return { clientX: LIST_LEFT + 24, clientY: listTop + index * ROW_HEIGHT + ROW_HEIGHT / 2 };
}

/** jsdom has no PointerEvent: a MouseEvent carrying the pointer's type. */
function pointerDown(el: Element, at: { clientX: number; clientY: number }) {
  const event = new MouseEvent('pointerdown', { bubbles: true, cancelable: true, ...at });
  Object.defineProperty(event, 'pointerType', { value: 'touch' });
  el.dispatchEvent(event);
}

function touchStart(el: Element, at: { clientX: number; clientY: number }) {
  const event = new Event('touchstart', { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'touches', { value: [{ target: el, ...at }] });
  el.dispatchEvent(event);
}

/** A finger landing on the title of the row at `index`. */
function touchTitle(index: number): HTMLElement {
  const code = titleCode(rowElements()[index]);
  const at = pointInRow(index);
  pointerDown(code, at);
  touchStart(code, at);
  return code;
}

/** The finger lifting. A replaced element's release reaches nothing else. */
function release(el: Element) {
  el.dispatchEvent(new Event('touchend', { bubbles: true, cancelable: true }));
}

function advance(ms: number) {
  vi.advanceTimersByTime(ms);
  flushSync();
}

function menu(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-slot="context-menu-content"]');
}

/** Opens nothing: neither our press nor bits-ui's own timer. */
function expectNoMenu() {
  advance(BITS_UI_LONG_PRESS_MS + LONG_PRESS_DELAY_MS);
  expect(menu()).toBeNull();
}

/** The menu is open, and it is the one for `key`'s row. */
function expectMenuFor(key: string) {
  const content = menu();
  if (!content) throw new Error('menu not open');
  const remove = [...content.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((el) =>
    el.textContent?.includes('Delete')
  );
  if (!remove) throw new Error('Delete not in the menu');
  remove.click();
  flushSync();
  expect(onDelete.mock.calls).toEqual([[key]]);
}

beforeEach(() => {
  vi.useFakeTimers();
  // The trigger's synthetic `contextmenu` passes `view: window`, and vitest's
  // `window` is not the jsdom Window that jsdom's MouseEvent insists on.
  const RealMouseEvent = MouseEvent;
  vi.stubGlobal(
    'MouseEvent',
    class extends RealMouseEvent {
      constructor(type: string, init: MouseEventInit = {}) {
        const { view: _view, ...rest } = init;
        super(type, rest);
      }
    }
  );
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    }
  );
  onDelete = vi.fn<(key: string) => void>();
  listTop = 200;
  pane = document.createElement('div');
  document.body.appendChild(pane);
  // jsdom lays nothing out. Rows stack from the top of the list in DOM
  // order, everything inside a row has the row's box, and the list's box
  // starts at its first row. Nothing else has a box, the `display: contents`
  // trigger included.
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    const row = this.closest('.timeline-row');
    const rows = rowElements();
    if (row && rows.includes(row)) {
      return new DOMRect(
        LIST_LEFT,
        listTop + rows.indexOf(row) * ROW_HEIGHT,
        LIST_WIDTH,
        ROW_HEIGHT
      );
    }
    if (this.matches('.timeline')) {
      return new DOMRect(LIST_LEFT, listTop, LIST_WIDTH, rows.length * ROW_HEIGHT);
    }
    return new DOMRect();
  });
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  pane.remove();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function threeRows(): TimelineWithMenuRow[] {
  return [
    { key: 'a', titleHtml: title('first') },
    { key: 'b', titleHtml: title('second') },
    { key: 'c', titleHtml: title('third') },
  ];
}

/** Re-renders a row's title, as a live title does: its elements are replaced. */
function retitle(props: { rows: TimelineWithMenuRow[] }, key: string, text: string) {
  props.rows = props.rows.map((row) =>
    row.key === key ? { ...row, titleHtml: title(text) } : row
  );
  flushSync();
}

/**
 * A row arrives above the others, and scroll anchoring holds those in view
 * where they are: the pane scrolls by the new row's height, so the top of
 * the list moves up by it.
 */
function rowArrivesAbove(props: { rows: TimelineWithMenuRow[] }) {
  props.rows = [{ key: 'new', titleHtml: title('arrived') }, ...props.rows];
  flushSync();
  listTop -= ROW_HEIGHT;
  pane.dispatchEvent(new Event('scroll'));
}

describe('TimelineContextMenu touch long press', () => {
  it('opens the menu for the row that was held', () => {
    render(threeRows());

    touchTitle(1);
    advance(LONG_PRESS_DELAY_MS);

    expectMenuFor('b');
  });

  it('opens the menu for the held row when its touched element was replaced mid-hold', () => {
    const props = render(threeRows());

    const code = touchTitle(1);
    advance(200);
    retitle(props, 'b', 'second, still streaming');
    expect(code.isConnected).toBe(false);
    expect(rowFor('b').isConnected).toBe(true);
    advance(LONG_PRESS_DELAY_MS - 200);

    expectMenuFor('b');
  });

  it('keeps the press when a row arrives above and the held row is anchored in place', () => {
    const props = render(threeRows());

    touchTitle(1);
    advance(200);
    retitle(props, 'b', 'second, still streaming');
    const heldAt = rowFor('b').getBoundingClientRect().top;
    rowArrivesAbove(props);
    // The list's top went up by a row; the held row is where it was.
    expect(pane.querySelector('.timeline')!.getBoundingClientRect().top).toBe(200 - ROW_HEIGHT);
    expect(rowFor('b').getBoundingClientRect().top).toBe(heldAt);
    expect(menu()).toBeNull();
    advance(LONG_PRESS_DELAY_MS - 200);

    expectMenuFor('b');
  });

  it('keeps the press through the same arrival when the touched element is still there', () => {
    const props = render(threeRows());

    touchTitle(1);
    advance(200);
    rowArrivesAbove(props);
    advance(LONG_PRESS_DELAY_MS - 200);

    expectMenuFor('b');
  });

  it('drops the press when a scroll carried the held row more than the tolerance', () => {
    const props = render(threeRows());

    touchTitle(1);
    advance(200);
    retitle(props, 'b', 'second, still streaming');
    listTop -= SCROLL_STILL_TOLERANCE_PX + 1;
    pane.dispatchEvent(new Event('scroll'));

    expectNoMenu();
  });

  it('keeps the press when a scroll left the held row within the tolerance', () => {
    const props = render(threeRows());

    touchTitle(1);
    advance(200);
    retitle(props, 'b', 'second, still streaming');
    listTop -= SCROLL_STILL_TOLERANCE_PX;
    pane.dispatchEvent(new Event('scroll'));
    advance(LONG_PRESS_DELAY_MS - 200);

    expectMenuFor('b');
  });

  it('opens nothing when the held row moved from under the finger with no scroll', () => {
    const props = render(threeRows());

    touchTitle(1);
    advance(200);
    retitle(props, 'b', 'second, still streaming');
    // A row arrives above with nothing to anchor: every row shifts down one,
    // no `scroll` is fired, and the finger is now on the row that was above.
    props.rows = [{ key: 'new', titleHtml: title('arrived') }, ...props.rows];
    flushSync();

    expectNoMenu();
    expect(onDelete).not.toHaveBeenCalled();
  });

  it('opens nothing when the held row was removed during the hold', () => {
    const props = render(threeRows());

    touchTitle(1);
    advance(200);
    props.rows = props.rows.filter((row) => row.key !== 'b');
    flushSync();

    expectNoMenu();
  });

  it("does not open an earlier touch's row for a hold on a row with no menu", () => {
    const props = render([
      { key: 'a', titleHtml: title('first') },
      { key: 'plain', titleHtml: title('no menu'), menu: false },
    ]);

    // A tap on the first row leaves its key behind: nothing closes a menu
    // that never opened.
    release(touchTitle(0));
    advance(1000);
    // The second row has no menu and stops its `pointerdown`, so the key is
    // still the first row's when this hold fires at the trigger.
    touchTitle(1);
    advance(200);
    retitle(props, 'plain', 'no menu, still streaming');

    expectNoMenu();
  });
});
