import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  guardTouchTaps,
  TAP_CLICK_WINDOW_MS,
  TAP_MAX_TRAVEL_PX,
  TAP_SCROLL_SLOP_PX,
} from './touchTapGuard';

// vitest runs in node, and a dispatched `Event` is never trusted, so the
// document is a stand-in that hands plain event objects to the capture
// listeners the guard registered. Positions are from the iOS 27.2 Safari
// trace of an accidental open: finger down at y=438, lifted at y=238.
//
// A touch with no `target` landed on nothing the guard can place, so only
// the 45 px limit applies to it. The scroller tests further down give the
// touch a stand-in element with the fields the guard reads: its parent, its
// document, its box and content sizes, and the three computed styles. One
// that a re-render took out of the document also says so (`isConnected`), and
// its document then answers `elementFromPoint`.

type Listener = (event: unknown) => void;

class FakeDocument {
  private listeners = new Map<string, Set<Listener>>();
  captureFlags: unknown[] = [];

  addEventListener(type: string, listener: Listener, options?: unknown): void {
    this.captureFlags.push(options);
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(listener);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  listenerCount(): number {
    let count = 0;
    for (const set of this.listeners.values()) count += set.size;
    return count;
  }

  emit(type: string, event: object): void {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

let doc: FakeDocument;
let now: number;

function pointer(
  type: 'pointerdown' | 'pointerup' | 'pointercancel',
  y: number,
  fields: {
    x?: number;
    pointerId?: number;
    pointerType?: string;
    isPrimary?: boolean;
    target?: unknown;
  } = {}
): void {
  doc.emit(type, {
    pointerId: fields.pointerId ?? 1,
    pointerType: fields.pointerType ?? 'touch',
    isPrimary: fields.isPrimary ?? true,
    clientX: fields.x ?? 200,
    clientY: y,
    timeStamp: now,
    target: fields.target ?? null,
  });
}

/** Deliver a click; returns whether the guard swallowed it. */
function click(isTrusted = true): boolean {
  const event = {
    isTrusted,
    timeStamp: now,
    preventDefault: vi.fn(),
    stopImmediatePropagation: vi.fn(),
  };
  doc.emit('click', event);
  const prevented = event.preventDefault.mock.calls.length > 0;
  expect(event.stopImmediatePropagation.mock.calls.length > 0).toBe(prevented);
  return prevented;
}

/** A touch from `fromY` to `toY`, lifted `durationMs` later. */
function touch(fromY: number, toY: number, durationMs = 60): void {
  pointer('pointerdown', fromY);
  now += durationMs;
  pointer('pointerup', toY);
}

/** A touch that lifts `dx` px across and `dy` px down from where it landed, on `target`. */
function travel(dx: number, dy: number, durationMs: number, target?: unknown): void {
  pointer('pointerdown', 438, { x: 200, target });
  now += durationMs;
  // The pointer is captured by where it landed; the guard reads the landing.
  pointer('pointerup', 438 + dy, { x: 200 + dx, target });
}

/** Whether the click of a touch on `target` that went `dx` across and `dy` down is swallowed. */
function flicked(target: unknown, dx: number, dy: number, durationMs = 50): boolean {
  travel(dx, dy, durationMs, target);
  now += 10;
  return click();
}

// ── Stand-in elements ────────────────────────────────────────────────────

interface Styles {
  overflowX: string;
  overflowY: string;
  touchAction: string;
}

interface FakeElement {
  nodeType: 1;
  name: string;
  parentElement: FakeElement | null;
  ownerDocument: FakePage;
  scrollWidth: number;
  clientWidth: number;
  scrollHeight: number;
  clientHeight: number;
  styles: Styles;
  isConnected?: boolean;
}

interface FakePage {
  documentElement: FakeElement;
  body: FakeElement;
  scrollingElement: FakeElement;
  elementFromPoint?: (x: number, y: number) => FakeElement | null;
}

type Box = Partial<
  Pick<FakeElement, 'scrollWidth' | 'clientWidth' | 'scrollHeight' | 'clientHeight'>
>;

/** An element under `parent`; with no sizes given its content fits its box. */
function element(
  name: string,
  parent: FakeElement,
  styles: Partial<Styles> = {},
  box: Box = {}
): FakeElement {
  return {
    nodeType: 1,
    name,
    parentElement: parent,
    ownerDocument: parent.ownerDocument,
    scrollWidth: 402,
    clientWidth: 402,
    scrollHeight: 100,
    clientHeight: 100,
    ...box,
    styles: { overflowX: 'visible', overflowY: 'visible', touchAction: 'auto', ...styles },
  };
}

/**
 * A document, with the app's `overflow: hidden` on <html> and <body> unless
 * told otherwise. Sizes are the phone layout's (402 x 646).
 */
function page(
  rootStyles: Partial<Styles> = { overflowX: 'hidden', overflowY: 'hidden' },
  box: Box = {}
) {
  const sized = { scrollHeight: 646, clientHeight: 646, ...box };
  const doc = {} as FakePage;
  const html = { ...element('html', { ownerDocument: doc } as FakeElement, rootStyles, sized) };
  html.parentElement = null;
  const body = element('body', html, rootStyles, sized);
  Object.assign(doc, { documentElement: html, body, scrollingElement: html });
  return { doc, html, body };
}

/**
 * The project list as the iOS 27.2 Simulator reports it: `.main-panel` is
 * `overflow-x: auto; overflow-y: scroll` with 3592 px of content in a 597 px
 * box and nothing wider than its 402 px. The New project button sits in the
 * top bar, outside it.
 */
function projectList() {
  const { doc, body } = page();
  const main = element('main', body, { overflowX: 'hidden', overflowY: 'hidden' });
  const topBar = element('top-bar', main);
  const newProject = element('new-project', topBar);
  const panel = element(
    'main-panel',
    main,
    { overflowX: 'auto', overflowY: 'scroll' },
    { scrollHeight: 3592, clientHeight: 597 }
  );
  const grid = element('projects-grid', panel, {}, { scrollHeight: 3560, clientHeight: 3560 });
  const card = element('card', grid);
  const title = element('card-title', card);
  return { doc, body, newProject, panel, grid, card, title };
}

/** A row of chips that scrolls sideways and not vertically, outside any other scroller. */
function chipRow() {
  const { body } = page();
  const row = element(
    'chip-row',
    body,
    { overflowX: 'auto', overflowY: 'hidden' },
    { scrollWidth: 1200, clientWidth: 402 }
  );
  const chip = element('chip', row);
  return { row, chip };
}

/**
 * Take `gone` out of its document, as a re-render during the touch does, and
 * leave `now` under every point of the page. Returns the page's
 * `elementFromPoint`.
 */
function replaced(gone: FakeElement, now: FakeElement | null) {
  Object.assign(gone, { isConnected: false, parentElement: null });
  const elementFromPoint = vi.fn((_x: number, _y: number) => now);
  gone.ownerDocument.elementFromPoint = elementFromPoint;
  return elementFromPoint;
}

function stubScale(scale: number | undefined): void {
  vi.stubGlobal('visualViewport', scale === undefined ? null : { scale });
}

/** Whether the click of a touch that travelled `px` in `durationMs` is swallowed, up and sideways. */
function swallowed(px: number, durationMs: number): boolean[] {
  travel(0, -px, durationMs);
  const vertical = click();
  travel(px, 0, durationMs);
  return [vertical, click()];
}

beforeEach(() => {
  doc = new FakeDocument();
  now = 1000;
  vi.stubGlobal('getComputedStyle', (el: FakeElement) => el.styles);
  guardTouchTaps(doc as unknown as EventTarget);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('guardTouchTaps', () => {
  it('swallows the click a flick ends in', () => {
    touch(438, 238);
    now += 10;
    expect(click()).toBe(true);
  });

  it('lets a plain tap click', () => {
    touch(438, 438);
    now += 10;
    expect(click()).toBe(false);
  });

  it('measures travel as a straight line, and only beyond the limit', () => {
    travel(0, TAP_MAX_TRAVEL_PX, 60);
    expect(click()).toBe(false);
    travel(0, TAP_MAX_TRAVEL_PX + 1, 60);
    expect(click()).toBe(true);

    // 30 px across and 30 px down is 42.4 px of travel; 32 and 32 is 45.3 px.
    travel(30, 30, 60);
    expect(click()).toBe(false);
    travel(32, 32, 60);
    expect(click()).toBe(true);
  });

  // The corners the rule has to hold at once: a pressing thumb rolls up to
  // about 30 px, quickly or slowly, and a touch that went far was no tap
  // however long it took.
  describe('the corners of the rule', () => {
    it('passes a 25 px roll lifted after 120 ms', () => {
      expect(swallowed(25, 120)).toEqual([false, false]);
    });

    it('passes a 30 px roll, quick or slow', () => {
      for (const durationMs of [30, 60, 120, 200, 400]) {
        expect(swallowed(30, durationMs), `${durationMs} ms`).toEqual([false, false]);
      }
    });

    it('swallows a 60 px flick lifted after 30 to 100 ms', () => {
      for (const durationMs of [30, 50, 80, 100]) {
        expect(swallowed(60, durationMs), `${durationMs} ms`).toEqual([true, true]);
      }
    });

    it('swallows an 80 px touch lifted after 250 ms', () => {
      expect(swallowed(80, 250)).toEqual([true, true]);
    });

    it('passes a press that rolls 10 to 20 px at any speed', () => {
      for (const px of [10, 15, 20]) {
        for (const durationMs of [20, 60, 120, 250, 1000]) {
          expect(swallowed(px, durationMs), `${px} px in ${durationMs} ms`).toEqual([false, false]);
        }
      }
    });

    it('swallows a far touch however slow', () => {
      for (const [px, durationMs] of [
        [60, 200],
        [100, 400],
        [200, 400],
        [200, 3000],
      ]) {
        expect(swallowed(px, durationMs), `${px} px in ${durationMs} ms`).toEqual([true, true]);
      }
    });
  });

  describe('along something that scrolls', () => {
    it('swallows a 24 px vertical flick on a card in a vertical scroller', () => {
      const { card, title } = projectList();
      expect(flicked(card, 0, -24, 50)).toBe(true);
      expect(flicked(card, 0, 24, 50)).toBe(true);
      // The touch lands on whatever is innermost; the scroller is found from there.
      expect(flicked(title, 0, -24, 50)).toBe(true);
    });

    it('passes the same roll sideways on that card', () => {
      // `.main-panel` is `overflow-x: auto` with nothing wider than its box.
      const { card, title } = projectList();
      expect(flicked(card, 24, 0, 50)).toBe(false);
      expect(flicked(card, -24, 0, 50)).toBe(false);
      expect(flicked(title, 26, 0, 120)).toBe(false);
      expect(flicked(card, 30, 0, 140)).toBe(false);
      expect(flicked(card, TAP_MAX_TRAVEL_PX, 0, 140)).toBe(false);
    });

    it('still swallows a sideways touch on the card past the 45 px limit', () => {
      const { card } = projectList();
      expect(flicked(card, TAP_MAX_TRAVEL_PX + 1, 0)).toBe(true);
      expect(flicked(card, 80, 0, 130)).toBe(true);
    });

    it('passes 24 and 30 px rolls on a button outside any scroller, either way', () => {
      const { newProject } = projectList();
      for (const px of [24, 30]) {
        for (const durationMs of [50, 120, 400]) {
          const label = `${px} px in ${durationMs} ms`;
          expect(flicked(newProject, 0, -px, durationMs), `${label} up`).toBe(false);
          expect(flicked(newProject, 0, px, durationMs), `${label} down`).toBe(false);
          expect(flicked(newProject, px, 0, durationMs), `${label} right`).toBe(false);
          expect(flicked(newProject, -px, 0, durationMs), `${label} left`).toBe(false);
        }
      }
    });

    it('swallows a sideways flick in a horizontal scroller and passes a vertical roll', () => {
      const { chip } = chipRow();
      expect(flicked(chip, 24, 0)).toBe(true);
      expect(flicked(chip, -24, 0)).toBe(true);
      expect(flicked(chip, 0, -24)).toBe(false);
      expect(flicked(chip, 0, 30, 120)).toBe(false);
    });

    it('starts at the distance iOS starts a scroll, whatever the duration', () => {
      const { card } = projectList();
      for (const durationMs of [20, 120, 600]) {
        expect(flicked(card, 0, -(TAP_SCROLL_SLOP_PX - 1), durationMs), `${durationMs} ms`).toBe(
          false
        );
        expect(flicked(card, 0, -TAP_SCROLL_SLOP_PX, durationMs), `${durationMs} ms`).toBe(true);
      }
      expect(flicked(card, 0, 0)).toBe(false);
    });

    it('swallows a diagonal flick on a card in a vertical scroller', () => {
      const { card, title } = projectList();
      // 26 px right and 24 px up is 35 pt, mostly sideways, and 24 pt of it
      // is along the list.
      expect(flicked(card, 26, -24, 50)).toBe(true);
      expect(flicked(title, 26, -24, 50)).toBe(true);
      expect(flicked(card, -26, 24, 50)).toBe(true);
      expect(flicked(card, 26, -24, 400)).toBe(true);
    });

    it('passes a sideways roll on that card that drifts along the list outside the cone', () => {
      const { card, title } = projectList();
      expect(flicked(card, 26, -8, 120)).toBe(false);
      expect(flicked(card, 26, 8, 120)).toBe(false);
      expect(flicked(title, -26, -9, 50)).toBe(false);
      expect(flicked(card, 26, -TAP_SCROLL_SLOP_PX, 120)).toBe(false);
      // 26 across and 11 along is 67° from the list's axis: iOS clicks for it.
      expect(flicked(card, 26, -(TAP_SCROLL_SLOP_PX + 1), 120)).toBe(false);
      // tan 30° of 26 is 15.01: 15 along is outside the cone, 16 inside.
      expect(flicked(card, 26, -15, 120)).toBe(false);
      expect(flicked(card, 26, -16, 120)).toBe(true);
      // 43 across and 10 along is 44.1 pt in all: still under the far limit.
      expect(flicked(card, 43, -10, 140)).toBe(false);
    });

    it('holds each axis to its own part of the travel', () => {
      const { card } = projectList();
      // 20 across and 12 down: 12 pt along the list, whichever part is larger.
      expect(flicked(card, 20, 12)).toBe(true);
      expect(flicked(card, 12, 20)).toBe(true);
      expect(flicked(card, -12, -20)).toBe(true);
      expect(flicked(card, 15, 15)).toBe(true);
      // 9 and 9 is 12.7 pt in all, and 9 pt along the list.
      expect(flicked(card, 9, 9)).toBe(false);
      expect(flicked(card, 9, -9)).toBe(false);
      const { chip } = chipRow();
      expect(flicked(chip, 20, 12)).toBe(true);
      // 12 along the row and 20 across it is inside the cone, as on the card.
      expect(flicked(chip, 12, 20)).toBe(true);
      // 11 along the row and 26 across it is not: the mirror of the card's roll.
      expect(flicked(chip, TAP_SCROLL_SLOP_PX + 1, 26)).toBe(false);
      expect(flicked(chip, 8, 26)).toBe(false);
      expect(flicked(chip, 9, 9)).toBe(false);
    });

    // Review baa3cc80: sampled strokes on a project card in the list, iOS 27.2
    // Simulator, as px across the list / px along it. With every sample
    // WebKit clicked for the first group and sent no click for the second, so
    // the guard must pass the one and swallow the other. A count is how many
    // times the review ran the stroke.
    const WEBKIT_CLICKED: [across: number, along: number][] = [
      [22, 12],
      [24, 12],
      [26, 12], // 4, up and down, left and right
      [26, 14],
      [30, 12], // 3
      [36, 20],
      [40, 12],
      [40, 14],
      [40, 18],
      [40, 20],
      // Not in the review's 25: 18/10 is its other reading of the cone's edge,
      // and 10/8 is short of the scroll limit.
      [18, 10],
      [10, 8],
    ];
    const WEBKIT_SCROLLED: [across: number, along: number][] = [
      [10, 10], // 2
      [12, 12],
      [14, 12],
      [16, 12],
      [20, 12], // 2
      [26, 16], // 2
      [34, 20],
      // The diagonal flick of bff08996, and the review's readings at the limit.
      [26, 24],
      [0, 10],
    ];
    const strokes = [
      ...WEBKIT_CLICKED.map(([across, along]) => ({ across, along, webkit: 'clicked' })),
      ...WEBKIT_SCROLLED.map(([across, along]) => ({ across, along, webkit: 'sent no click' })),
    ];

    describe.each([
      {
        where: 'a card in a vertical scroller',
        landing: () => projectList().card,
        // Across the list is x, along it is y.
        delta: (across: number, along: number) => [across, along],
      },
      {
        where: 'a chip in a horizontal scroller',
        landing: () => chipRow().chip,
        delta: (across: number, along: number) => [along, across],
      },
    ])('the strokes WebKit was measured on, on $where', ({ landing, delta }) => {
      it.each(strokes)('$across across, $along along: WebKit $webkit', (stroke) => {
        const target = landing();
        const [dx, dy] = delta(stroke.across, stroke.along);
        // Either way along and either way across, quick and slow.
        for (const [sx, sy] of [
          [1, 1],
          [1, -1],
          [-1, 1],
          [-1, -1],
        ]) {
          for (const durationMs of [50, 160, 610]) {
            expect(
              flicked(target, sx * dx, sy * dy, durationMs),
              `${sx * dx}, ${sy * dy} in ${durationMs} ms`
            ).toBe(stroke.webkit === 'sent no click');
          }
        }
      });
    });

    it('passes diagonal rolls on a button outside any scroller', () => {
      const { newProject } = projectList();
      for (const durationMs of [50, 120, 400]) {
        expect(flicked(newProject, 26, -24, durationMs), `${durationMs} ms`).toBe(false);
        expect(flicked(newProject, -26, 24, durationMs), `${durationMs} ms`).toBe(false);
        expect(flicked(newProject, 24, 26, durationMs), `${durationMs} ms`).toBe(false);
        expect(flicked(newProject, 30, 30, durationMs), `${durationMs} ms`).toBe(false);
      }
    });

    it('swallows either way where an outer scroller takes the other axis', () => {
      // A board: columns that scroll vertically inside a board that pages sideways.
      const { body } = page();
      const board = element(
        'board',
        body,
        { overflowX: 'auto', overflowY: 'hidden' },
        { scrollWidth: 1608, clientWidth: 402 }
      );
      const column = element(
        'column',
        board,
        { overflowX: 'hidden', overflowY: 'auto' },
        { scrollHeight: 1400, clientHeight: 500 }
      );
      const card = element('card', column);
      expect(flicked(card, 0, -24)).toBe(true);
      expect(flicked(card, 24, 0)).toBe(true);
    });

    it('does not count a scroller with nothing to scroll', () => {
      const { body } = page();
      const pane = element('pane', body, { overflowX: 'auto', overflowY: 'auto' });
      const button = element('button', pane);
      expect(flicked(button, 0, -24)).toBe(false);
      expect(flicked(button, 24, 0)).toBe(false);
    });

    it('does not count clipped content a finger cannot scroll', () => {
      const { body } = page();
      for (const overflowY of ['hidden', 'clip', 'visible']) {
        const pane = element('pane', body, { overflowY }, { scrollHeight: 900, clientHeight: 300 });
        expect(flicked(element('button', pane), 0, -24), overflowY).toBe(false);
      }
    });

    it('counts a scroller whatever it is scrolled to', () => {
      // At an end iOS still takes the touch, to rubber-band or scroll what is outside.
      const { panel, card } = projectList();
      for (const scrollTop of [0, 1200, 3592 - 597]) {
        Object.assign(panel, { scrollTop });
        expect(flicked(card, 0, 24), `finger down at ${scrollTop}`).toBe(true);
        expect(flicked(card, 0, -24), `finger up at ${scrollTop}`).toBe(true);
      }
    });

    it('stops at an element whose touch-action rules out the pan', () => {
      const { grid } = projectList();
      const cases: [string, boolean, boolean][] = [
        // touch-action, vertical flick swallowed, sideways roll swallowed
        ['auto', true, false],
        ['manipulation', true, false],
        ['none', false, false],
        ['pinch-zoom', false, false],
        ['pan-x', false, false],
        ['pan-y', true, false],
        ['pan-x pan-y', true, false],
        ['pan-y pinch-zoom', true, false],
        ['pan-up', true, false],
      ];
      for (const [touchAction, vertical, sideways] of cases) {
        const handle = element('handle', grid, { touchAction });
        const grip = element('grip', handle);
        expect(flicked(grip, 0, -24), `${touchAction}, vertical`).toBe(vertical);
        expect(flicked(grip, 24, 0), `${touchAction}, sideways`).toBe(sideways);
      }
      const { row } = chipRow();
      expect(flicked(element('chip', row, { touchAction: 'pan-y' }), 24, 0)).toBe(false);
      expect(flicked(element('chip', row, { touchAction: 'pan-x' }), 24, 0)).toBe(true);
    });

    it("never counts the app's document, which is overflow: hidden", () => {
      // Content taller than the viewport, as while a keyboard is up.
      const { body } = page(
        { overflowX: 'hidden', overflowY: 'hidden' },
        { scrollHeight: 900, clientHeight: 646 }
      );
      const button = element('button', body);
      expect(flicked(button, 0, -24)).toBe(false);
      expect(flicked(button, 24, 0)).toBe(false);
      expect(flicked(body, 0, -24)).toBe(false);
    });

    it('counts a document left to scroll, on the axis its content overflows', () => {
      for (const overflowY of ['visible', 'auto', 'scroll']) {
        const { body, html } = page(
          { overflowX: 'visible', overflowY },
          { scrollHeight: 2000, clientHeight: 646 }
        );
        const button = element('button', body);
        expect(flicked(button, 0, -24), overflowY).toBe(true);
        expect(flicked(button, 24, 0), overflowY).toBe(false);
        expect(flicked(html, 0, -24), overflowY).toBe(true);
      }
      // Nothing below the fold: nothing to scroll.
      const fits = page({ overflowX: 'visible', overflowY: 'visible' });
      expect(flicked(element('button', fits.body), 0, -24)).toBe(false);
    });

    it("takes the viewport's overflow from <body> where <html> leaves it visible", () => {
      const { body, html } = page({}, { scrollHeight: 2000, clientHeight: 646 });
      body.styles.overflowY = 'hidden';
      const button = element('button', body);
      expect(flicked(button, 0, -24)).toBe(false);
      body.styles.overflowY = 'visible';
      html.styles.overflowY = 'clip';
      expect(flicked(button, 0, -24)).toBe(false);
      html.styles.overflowY = 'visible';
      expect(flicked(button, 0, -24)).toBe(true);
    });

    it('passes a roll on an element that left the document during the touch', () => {
      const { grid } = projectList();
      const card = element('card', grid);
      card.parentElement = null;
      expect(flicked(card, 0, -24)).toBe(false);
      // Past the 45 px limit nothing is looked up.
      expect(flicked(card, 0, -60)).toBe(true);
    });

    it('looks from the element now at the press point when the touched one was replaced', () => {
      // A live title re-rendered under the finger: the touched element is
      // out of the document, and its replacement sits in the same card.
      const { card, title } = projectList();
      const rerendered = element('card-title', card);
      const elementFromPoint = replaced(title, rerendered);
      expect(flicked(title, 0, -24)).toBe(true);
      // Asked for the point the finger landed on, not where it lifted.
      expect(elementFromPoint).toHaveBeenLastCalledWith(200, 438);
      expect(flicked(title, 26, -24)).toBe(true);
      // The replacement is held to the same limits as an element still there.
      expect(flicked(title, 24, 0)).toBe(false);
      expect(flicked(title, 26, -8, 120)).toBe(false);
      expect(flicked(title, 0, -(TAP_SCROLL_SLOP_PX - 1))).toBe(false);
      expect(flicked(title, 0, -TAP_SCROLL_SLOP_PX)).toBe(true);
    });

    it('passes a roll on a replaced element with no scroller at the press point', () => {
      const { newProject, title } = projectList();
      const elementFromPoint = replaced(title, newProject);
      expect(flicked(title, 0, -24)).toBe(false);
      expect(flicked(title, 26, -24)).toBe(false);
      // Nothing at the point at all, as outside the viewport.
      elementFromPoint.mockReturnValue(null);
      expect(flicked(title, 0, -24)).toBe(false);
    });

    it('asks for the element at the press point only when the touched one is gone', () => {
      const { doc, card, title } = projectList();
      const elementFromPoint = vi.fn(() => null);
      doc.elementFromPoint = elementFromPoint;
      expect(flicked(title, 0, -24)).toBe(true);
      expect(elementFromPoint).not.toHaveBeenCalled();
      // Gone, but a tap, or past the 45 px limit: nothing is looked up.
      replaced(title, card);
      expect(flicked(title, 0, -8)).toBe(false);
      expect(flicked(title, 0, -60)).toBe(true);
      expect(title.ownerDocument.elementFromPoint).not.toHaveBeenCalled();
    });

    it('passes a roll that landed on something that is not an element', () => {
      expect(flicked({ nodeType: 9 }, 0, -24)).toBe(false);
      expect(flicked(undefined, 0, -24)).toBe(false);
    });

    it('reads no styles for a tap or a far flick', () => {
      const { card } = projectList();
      const read = vi.fn((el: FakeElement) => el.styles);
      vi.stubGlobal('getComputedStyle', read);
      expect(flicked(card, 0, 0)).toBe(false);
      expect(flicked(card, 0, -8)).toBe(false);
      expect(flicked(card, 0, -200)).toBe(true);
      expect(read).not.toHaveBeenCalled();
      expect(flicked(card, 0, -24)).toBe(true);
      expect(read).toHaveBeenCalled();
    });
  });

  // Pointer positions are layout px. Pinch-zoomed to 2x, each is two points
  // on the screen, and both limits are in points.
  describe('while pinch-zoomed', () => {
    it('holds the 45 pt limit in screen points', () => {
      const { newProject } = projectList();
      stubScale(2);
      // 30 px is 60 pt: a flick. 22 px is 44 pt: a roll.
      expect(flicked(newProject, 30, 0)).toBe(true);
      expect(flicked(newProject, 0, -30)).toBe(true);
      expect(flicked(newProject, 22, 0)).toBe(false);
      expect(flicked(newProject, 0, -23)).toBe(true);
      stubScale(1);
      expect(flicked(newProject, 30, 0)).toBe(false);
      expect(flicked(newProject, 0, -30)).toBe(false);
    });

    it('holds the 10 pt scroll limit in screen points', () => {
      const { card } = projectList();
      stubScale(2);
      // 6 px is 12 pt along the list, 5 px is 10 pt, and 4 px is 8 pt.
      expect(flicked(card, 0, -6)).toBe(true);
      expect(flicked(card, 0, -5)).toBe(true);
      expect(flicked(card, 0, -4)).toBe(false);
      stubScale(3);
      expect(flicked(card, 0, -4)).toBe(true);
      expect(flicked(card, 14, 0)).toBe(false);
      expect(flicked(card, 16, 0)).toBe(true);
      stubScale(1);
      expect(flicked(card, 0, -6)).toBe(false);
    });

    it('holds each axis to the scroll limit in screen points', () => {
      const { card } = projectList();
      stubScale(2);
      // 13 px across and 12 px up is 26 pt and 24 pt: along the list, inside
      // the cone. 13 and 6 is 26 pt and 12 pt: outside it at any scale.
      expect(flicked(card, 13, -12)).toBe(true);
      expect(flicked(card, 13, -6)).toBe(false);
      // 5 px each way is 10 pt along the list; 4 px is 8 pt.
      expect(flicked(card, 5, -5)).toBe(true);
      expect(flicked(card, 5, -4)).toBe(false);
      stubScale(1);
      expect(flicked(card, 13, -6)).toBe(false);
      expect(flicked(card, 5, -5)).toBe(false);
    });

    it('measures in layout px where the scale cannot be read', () => {
      const { newProject, card } = projectList();
      for (const scale of [undefined, 0, Number.NaN]) {
        stubScale(scale);
        expect(flicked(newProject, 30, 0), String(scale)).toBe(false);
        expect(flicked(newProject, 46, 0), String(scale)).toBe(true);
        expect(flicked(card, 0, -9), String(scale)).toBe(false);
        expect(flicked(card, 0, -10), String(scale)).toBe(true);
      }
    });
  });

  it('swallows only one click per travelled touch', () => {
    touch(438, 238);
    expect(click()).toBe(true);
    expect(click()).toBe(false);
  });

  it('stops waiting once the click window has passed', () => {
    touch(438, 238);
    now += TAP_CLICK_WINDOW_MS + 1;
    expect(click()).toBe(false);
  });

  it('still swallows a click at the end of the window', () => {
    touch(438, 238);
    now += TAP_CLICK_WINDOW_MS;
    expect(click()).toBe(true);
  });

  it('stops waiting when the next press begins', () => {
    touch(438, 238);
    now += 100;
    touch(300, 300);
    expect(click()).toBe(false);
  });

  it('stops waiting when a mouse press begins', () => {
    touch(438, 238);
    pointer('pointerdown', 300, { pointerId: 9, pointerType: 'mouse' });
    pointer('pointerup', 300, { pointerId: 9, pointerType: 'mouse' });
    expect(click()).toBe(false);
  });

  it('forgets a touch the browser cancelled to scroll', () => {
    pointer('pointerdown', 438);
    pointer('pointercancel', 420);
    // A stray pointerup for the cancelled pointer must not arm the guard.
    pointer('pointerup', 238);
    expect(click()).toBe(false);
  });

  it('ignores a cancel for another pointer', () => {
    pointer('pointerdown', 438);
    pointer('pointercancel', 438, { pointerId: 2, isPrimary: false });
    pointer('pointerup', 238);
    expect(click()).toBe(true);
  });

  it('ignores mouse and pen drags', () => {
    for (const pointerType of ['mouse', 'pen']) {
      pointer('pointerdown', 438, { pointerType });
      pointer('pointerup', 238, { pointerType });
      expect(click()).toBe(false);
    }
  });

  it('ignores a second finger', () => {
    pointer('pointerdown', 438, { pointerId: 2, isPrimary: false });
    pointer('pointerup', 238, { pointerId: 2, isPrimary: false });
    expect(click()).toBe(false);
  });

  it("measures the primary touch against its own pointerup, not another finger's", () => {
    pointer('pointerdown', 438);
    pointer('pointerup', 100, { pointerId: 2, isPrimary: false });
    expect(click()).toBe(false);
    pointer('pointerup', 238);
    expect(click()).toBe(true);
  });

  it('passes a scripted click through and keeps waiting for the real one', () => {
    touch(438, 238);
    expect(click(false)).toBe(false);
    expect(click()).toBe(true);
  });

  it('listens on the capture phase and removes every listener on cleanup', () => {
    const other = new FakeDocument();
    const stop = guardTouchTaps(other as unknown as EventTarget);
    expect(other.captureFlags).toEqual([true, true, true, true]);
    expect(other.listenerCount()).toBe(4);
    stop();
    expect(other.listenerCount()).toBe(0);
  });

  it('is a no-op without a document', () => {
    expect(() => guardTouchTaps()()).not.toThrow();
  });
});
