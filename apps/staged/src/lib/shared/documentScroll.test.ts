import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { guardDocumentScroll, resetDocumentScroll } from './documentScroll';

// vitest runs in node: `window`, `document` and `<body>` are stubbed down to
// what the module touches. Numbers are iOS 27.2 Safari's: closing a dialog
// left `<body>` scrolled by the 40 px `100vh` overflowed it by (issue 02).

let fakeWindow: EventTarget & {
  visualViewport: { scale: number } | undefined;
  scrollX: number;
  scrollY: number;
  scrollTo: ReturnType<typeof vi.fn>;
};
let body: EventTarget & { scrollTop: number; scrollLeft: number };
let fakeDocument: { body: typeof body; activeElement: unknown };

function element(tagName: string, inFullScreenDialog = false) {
  return {
    tagName,
    closest: (selector: string) =>
      selector === '[data-full-screen]' && inFullScreenDialog ? {} : null,
  };
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
    visualViewport: { scale: 1 },
    scrollX: 0,
    scrollY: 0,
    scrollTo: vi.fn((x: number, y: number) => {
      fakeWindow.scrollX = x;
      fakeWindow.scrollY = y;
    }),
  });
  body = Object.assign(new EventTarget(), { scrollTop: 0, scrollLeft: 0 });
  fakeDocument = { body, activeElement: element('BUTTON') };
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

    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
  });

  it('undoes it for an input in a full-screen dialog, which fits above the keyboard itself', () => {
    fakeDocument.activeElement = element('TEXTAREA', true);
    guardDocumentScroll();

    scrollWindow(330);

    expect(fakeWindow.scrollY).toBe(0);
  });

  it('leaves a pinch-zoomed page where the user panned it', () => {
    fakeWindow.visualViewport = { scale: 2 };
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

    expect(body.scrollTop).toBe(40);
    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
  });
});
