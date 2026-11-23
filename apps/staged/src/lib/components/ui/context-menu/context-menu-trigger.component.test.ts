import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import TriggerWithMenu from './testStubs/TriggerWithMenu.svelte';
import { LONG_PRESS_DELAY_MS } from './longPress';
import { reactiveProps } from '$lib/features/sessions/testStubs/reactiveProps.svelte';

// bits-ui's ContextMenuTrigger opens the menu 700 ms after a touch
// `pointerdown` unless a pointermove, pointerup or pointercancel reaches the
// trigger first. Touch long presses belong to longPress.ts, which opens the
// menu through `contextmenu`; when it declines or drops a press, bits-ui's
// timer must not open the menu behind its back.

const BITS_UI_LONG_PRESS_MS = 700;

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

function render(extra: { disabled?: boolean; onpointerdown?: (e: PointerEvent) => void } = {}) {
  const props = reactiveProps({ open: false, ...extra });
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(TriggerWithMenu, { target, props });
  flushSync();
  const row = target.querySelector<HTMLElement>('[data-testid="row"]');
  if (!row) throw new Error('row not rendered');
  return { props, row };
}

/** jsdom has no PointerEvent: a MouseEvent carrying the pointer's type. */
function pointerDown(el: Element, pointerType: string) {
  const event = new MouseEvent('pointerdown', {
    bubbles: true,
    cancelable: true,
    clientX: 40,
    clientY: 40,
  });
  Object.defineProperty(event, 'pointerType', { value: pointerType });
  el.dispatchEvent(event);
  return event;
}

function touchStart(el: Element) {
  const event = new Event('touchstart', { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'touches', {
    value: [{ target: el, clientX: 40, clientY: 40 }],
  });
  el.dispatchEvent(event);
}

function advance(ms: number) {
  vi.advanceTimersByTime(ms);
  flushSync();
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
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  target.remove();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('ContextMenu.Trigger touch presses', () => {
  it('stays closed when a touch press is held with no contextmenu', () => {
    const { props, row } = render();

    // A pointerdown our controller never hears a touchstart for stands in for
    // a press it declined: nothing of ours will dispatch `contextmenu`.
    const event = pointerDown(row, 'touch');
    advance(BITS_UI_LONG_PRESS_MS + 100);

    expect(event.defaultPrevented).toBe(true);
    expect(props.open).toBe(false);
  });

  it('stays closed when the list scrolls under a held finger', () => {
    const { props, row } = render();

    pointerDown(row, 'touch');
    touchStart(row);
    advance(LONG_PRESS_DELAY_MS - 100);
    document.dispatchEvent(new Event('scroll'));
    advance(BITS_UI_LONG_PRESS_MS);

    expect(props.open).toBe(false);
  });

  it('opens when a scroll under a held finger leaves the row where it is', () => {
    const { props, row } = render();
    // jsdom lays nothing out: give the row a box that a scroll-anchoring
    // adjustment does not move.
    row.getBoundingClientRect = () => new DOMRect(16, 300, 320, 44);

    pointerDown(row, 'touch');
    touchStart(row);
    advance(LONG_PRESS_DELAY_MS - 100);
    document.dispatchEvent(new Event('scroll'));
    expect(props.open).toBe(false);
    advance(100);

    expect(props.open).toBe(true);
  });

  it('stays closed when the scroll carried the row with it', () => {
    const { props, row } = render();
    let top = 300;
    row.getBoundingClientRect = () => new DOMRect(16, top, 320, 44);

    pointerDown(row, 'touch');
    touchStart(row);
    advance(LONG_PRESS_DELAY_MS - 100);
    top = 280;
    document.dispatchEvent(new Event('scroll'));
    advance(BITS_UI_LONG_PRESS_MS);

    expect(props.open).toBe(false);
  });

  // The triggers that wrap one row: the project card, the sidebar row and the
  // board card. A re-render inside the row replaces the element under the
  // finger, and the row itself stays.
  function touchLabel(row: HTMLElement): HTMLElement {
    const label = row.querySelector<HTMLElement>('[data-testid="label"]');
    if (!label) throw new Error('label not rendered');
    label.getBoundingClientRect = () => row.getBoundingClientRect();
    pointerDown(label, 'touch');
    touchStart(label);
    return label;
  }

  it('opens when the touched element was replaced during the hold and a scroll leaves the row where it is', () => {
    const { props, row } = render();
    let top = 300;
    row.getBoundingClientRect = () => new DOMRect(16, top, 320, 44);

    const label = touchLabel(row);
    advance(LONG_PRESS_DELAY_MS - 100);
    label.replaceWith(document.createElement('span'));
    top = 300.5;
    document.dispatchEvent(new Event('scroll'));
    expect(props.open).toBe(false);
    advance(100);

    expect(props.open).toBe(true);
  });

  it('stays closed when the touched element was replaced during the hold and a scroll carried the row', () => {
    const { props, row } = render();
    let top = 300;
    row.getBoundingClientRect = () => new DOMRect(16, top, 320, 44);

    const label = touchLabel(row);
    advance(LONG_PRESS_DELAY_MS - 100);
    label.replaceWith(document.createElement('span'));
    top = 280;
    document.dispatchEvent(new Event('scroll'));
    advance(BITS_UI_LONG_PRESS_MS);

    expect(props.open).toBe(false);
  });

  it('opens when the touched element was replaced during the hold and nothing scrolls', () => {
    const { props, row } = render();

    const label = touchLabel(row);
    advance(LONG_PRESS_DELAY_MS - 100);
    label.replaceWith(document.createElement('span'));
    advance(100);

    expect(props.open).toBe(true);
  });

  it('stays closed when the whole row was replaced during the hold and the list scrolls', () => {
    const { props, row } = render();
    row.getBoundingClientRect = () => new DOMRect(16, 300, 320, 44);

    touchLabel(row);
    advance(LONG_PRESS_DELAY_MS - 100);
    // Nothing the finger landed in is left, so there is nothing to say the
    // new row is where the old one was.
    const next = document.createElement('div');
    next.getBoundingClientRect = () => new DOMRect(16, 300, 320, 44);
    row.replaceWith(next);
    document.dispatchEvent(new Event('scroll'));
    advance(BITS_UI_LONG_PRESS_MS);

    expect(props.open).toBe(false);
  });

  it('stays closed for a pen press held with no contextmenu', () => {
    const { props, row } = render();

    pointerDown(row, 'pen');
    advance(BITS_UI_LONG_PRESS_MS + 100);

    expect(props.open).toBe(false);
  });

  it('opens on a stationary touch hold, through our controller', () => {
    const { props, row } = render();

    pointerDown(row, 'touch');
    touchStart(row);
    advance(LONG_PRESS_DELAY_MS - 1);
    expect(props.open).toBe(false);
    advance(1);

    expect(props.open).toBe(true);
  });

  it('opens on a right-click', () => {
    const { props, row } = render();

    const down = pointerDown(row, 'mouse');
    row.dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 40, clientY: 40 })
    );
    flushSync();

    expect(down.defaultPrevented).toBe(false);
    expect(props.open).toBe(true);
  });

  it("runs the consumer's pointerdown handler first", () => {
    const seen: boolean[] = [];
    const { row } = render({ onpointerdown: (e) => seen.push(e.defaultPrevented) });

    pointerDown(row, 'touch');

    expect(seen).toEqual([false]);
  });

  it('leaves a disabled trigger alone', () => {
    const { props, row } = render({ disabled: true });

    const event = pointerDown(row, 'touch');
    advance(BITS_UI_LONG_PRESS_MS + 100);

    expect(event.defaultPrevented).toBe(false);
    expect(props.open).toBe(false);
  });
});
