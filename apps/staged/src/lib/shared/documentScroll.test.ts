import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  guardDocumentScroll,
  resetDocumentScroll,
  shouldKeepDocumentScroll,
} from './documentScroll';

// vitest runs in node: `window`, `document` and `<body>` are stubbed down to
// what the module touches. Numbers are iOS 27.2 Safari's: closing a dialog
// left `<body>` scrolled by the 40 px `100vh` overflowed it by (issue 02);
// a 714 px layout viewport, 384 px visible with the keyboard up.

class FakeVisualViewport extends EventTarget {
  height = 714;
  scale = 1;
}

let fakeWindow: EventTarget & {
  visualViewport: FakeVisualViewport | undefined;
  scrollX: number;
  scrollY: number;
  scrollTo: ReturnType<typeof vi.fn>;
};
let body: EventTarget & { scrollTop: number; scrollLeft: number };
let fakeDocument: {
  body: typeof body;
  documentElement: { clientHeight: number };
  activeElement: unknown;
};

/**
 * A stand-in element. `pageTop` is where it sits on the unscrolled page, so
 * its client rect moves with the scroll; the default is low on the page,
 * where the keyboard would cover it.
 */
function element(tagName: string, inFullScreenDialog = false, pageTop = 600) {
  return {
    tagName,
    closest: (selector: string) =>
      selector === '[data-full-screen]' && inFullScreenDialog ? {} : null,
    getBoundingClientRect: () => {
      const top = pageTop - fakeWindow.scrollY - body.scrollTop;
      return { top, bottom: top + 30 };
    },
  };
}

function raiseKeyboard(): void {
  fakeWindow.visualViewport!.height = 384;
  fakeWindow.visualViewport!.dispatchEvent(new Event('resize'));
}

function scrollBody(by: number): void {
  body.scrollTop = by;
  body.dispatchEvent(new Event('scroll'));
}

function scrollWindow(by: number): void {
  fakeWindow.scrollY = by;
  fakeWindow.dispatchEvent(new Event('scroll'));
}

beforeEach(() => {
  fakeWindow = Object.assign(new EventTarget(), {
    visualViewport: new FakeVisualViewport(),
    scrollX: 0,
    scrollY: 0,
    scrollTo: vi.fn((x: number, y: number) => {
      fakeWindow.scrollX = x;
      fakeWindow.scrollY = y;
    }),
  });
  body = Object.assign(new EventTarget(), { scrollTop: 0, scrollLeft: 0 });
  fakeDocument = { body, documentElement: { clientHeight: 714 }, activeElement: element('BUTTON') };
  vi.stubGlobal('window', fakeWindow);
  vi.stubGlobal('document', fakeDocument);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('resetDocumentScroll', () => {
  it('scrolls the window and body back to the origin', () => {
    fakeWindow.scrollY = 330;
    body.scrollTop = 40;
    body.scrollLeft = 4;

    resetDocumentScroll();

    expect(fakeWindow.scrollTo).toHaveBeenCalledWith(0, 0);
    expect(body).toMatchObject({ scrollTop: 0, scrollLeft: 0 });
  });

  it('writes nothing at the origin', () => {
    resetDocumentScroll();
    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
  });
});

describe('shouldKeepDocumentScroll', () => {
  it('keeps the scroll for a focused input outside a full-screen dialog', () => {
    fakeDocument.activeElement = element('INPUT');
    expect(shouldKeepDocumentScroll()).toBe(true);

    // Still needed once the keyboard is up: it would cover the input.
    fakeWindow.visualViewport!.height = 384;
    fakeWindow.scrollY = 330;
    expect(shouldKeepDocumentScroll()).toBe(true);
  });

  it('gives it up once the keyboard is up, if the input shows without it', () => {
    // The project list's search field, near the top: Safari scrolled 5 px.
    fakeDocument.activeElement = element('INPUT', false, 100);
    fakeWindow.scrollY = 5;
    expect(shouldKeepDocumentScroll()).toBe(true);

    fakeWindow.visualViewport!.height = 384;
    expect(shouldKeepDocumentScroll()).toBe(false);
  });

  it('gives it up for an input in a full-screen dialog, or with nothing to type into', () => {
    fakeDocument.activeElement = element('TEXTAREA', true);
    expect(shouldKeepDocumentScroll()).toBe(false);

    fakeDocument.activeElement = element('BUTTON');
    expect(shouldKeepDocumentScroll()).toBe(false);
  });

  it('keeps a pinch-zoomed page', () => {
    fakeWindow.visualViewport!.scale = 2;
    expect(shouldKeepDocumentScroll()).toBe(true);
  });
});

describe('guardDocumentScroll', () => {
  it('undoes a body scroll as it happens', () => {
    guardDocumentScroll();
    scrollBody(40);
    expect(body.scrollTop).toBe(0);
  });

  it('undoes a window scroll as it happens', () => {
    guardDocumentScroll();
    scrollWindow(40);
    expect(fakeWindow.scrollTo).toHaveBeenCalledWith(0, 0);
    expect(fakeWindow.scrollY).toBe(0);
  });

  it('leaves the scroll that reveals a focused input above the keyboard', () => {
    fakeDocument.activeElement = element('INPUT');
    guardDocumentScroll();

    scrollWindow(330);
    raiseKeyboard();

    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
  });

  it('undoes a scroll the input turns out not to need once the keyboard is up', () => {
    fakeDocument.activeElement = element('INPUT', false, 100);
    guardDocumentScroll();

    // Safari scrolls as the keyboard starts to come up.
    scrollWindow(5);
    expect(fakeWindow.scrollY).toBe(5);

    raiseKeyboard();
    expect(fakeWindow.scrollY).toBe(0);
  });

  it('undoes it for an input in a full-screen dialog, which fits above the keyboard itself', () => {
    fakeDocument.activeElement = element('TEXTAREA', true);
    guardDocumentScroll();

    scrollWindow(330);

    expect(fakeWindow.scrollY).toBe(0);
  });

  it('leaves a pinch-zoomed page where the user panned it', () => {
    fakeWindow.visualViewport!.scale = 2;
    guardDocumentScroll();

    scrollWindow(120);

    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
  });

  it('works without visualViewport', () => {
    fakeWindow.visualViewport = undefined;
    guardDocumentScroll();
    scrollBody(40);
    expect(body.scrollTop).toBe(0);
  });

  it('stops guarding once cleaned up', () => {
    const unguard = guardDocumentScroll();
    unguard();

    scrollBody(40);
    scrollWindow(40);
    raiseKeyboard();

    expect(body.scrollTop).toBe(40);
    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
  });
});
