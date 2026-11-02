import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The module's `$state` rune compiles away in the app build; under vitest it
// is a plain global call, so stub it as identity (preferences.test.ts
// precedent). vitest runs in node here, so `window`/`document` are stubbed
// just far enough for the module to drive them, with iOS 27.2 Safari's
// numbers: a 714 px layout viewport, 384 px visible once the keyboard is up.

class FakeVisualViewport extends EventTarget {
  height = 714;
  offsetTop = 0;
  scale = 1;
}

let viewport: FakeVisualViewport;
let fakeWindow: {
  visualViewport: FakeVisualViewport;
  scrollX: number;
  scrollY: number;
  scrollTo: ReturnType<typeof vi.fn>;
};
let body: { scrollTop: number; scrollLeft: number };
let fakeDocument: EventTarget & { activeElement: unknown };
let rafCallbacks: Array<() => void>;

const composer = { tagName: 'TEXTAREA' };

function flushFrames(): void {
  const pending = rafCallbacks;
  rafCallbacks = [];
  for (const cb of pending) cb();
}

/** What Safari does when an input low on the page takes focus. */
function openKeyboardWithPageScroll(): void {
  fakeDocument.activeElement = composer;
  viewport.height = 384;
  viewport.offsetTop = 330;
  fakeWindow.scrollY = 330;
  viewport.dispatchEvent(new Event('resize'));
}

async function importModule() {
  return await import('./visualViewport.svelte');
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  viewport = new FakeVisualViewport();
  fakeWindow = {
    visualViewport: viewport,
    scrollX: 0,
    scrollY: 0,
    // Scrolling the page carries the visual viewport with it.
    scrollTo: vi.fn((x: number, y: number) => {
      fakeWindow.scrollX = x;
      fakeWindow.scrollY = y;
      viewport.offsetTop = y;
    }),
  };
  body = { scrollTop: 0, scrollLeft: 0 };
  rafCallbacks = [];
  vi.stubGlobal('window', fakeWindow);
  fakeDocument = Object.assign(new EventTarget(), {
    documentElement: { clientHeight: 714 },
    body,
    activeElement: composer as unknown,
  });
  vi.stubGlobal('document', fakeDocument);
  vi.stubGlobal('requestAnimationFrame', (cb: () => void) => {
    rafCallbacks.push(cb);
    return rafCallbacks.length;
  });
  vi.stubGlobal('cancelAnimationFrame', () => {});
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('watchVisualViewport', () => {
  it('holds the layout geometry until the deferred first read', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    expect(visibleViewport).toEqual({ top: 0, height: null, keyboardOpen: false, zoomed: false });

    flushFrames();
    expect(visibleViewport).toEqual({ top: 0, height: 714, keyboardOpen: false, zoomed: false });
  });

  it('undoes the page scroll Safari adds for the keyboard and pins to what is left', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();

    openKeyboardWithPageScroll();

    expect(fakeWindow.scrollTo).toHaveBeenCalledWith(0, 0);
    expect(visibleViewport).toEqual({ top: 0, height: 384, keyboardOpen: true, zoomed: false });
  });

  it('follows the visual viewport if the page scroll could not be undone', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    fakeWindow.scrollTo.mockImplementation(() => {});
    watchVisualViewport();
    flushFrames();

    openKeyboardWithPageScroll();

    expect(visibleViewport).toEqual({ top: 330, height: 384, keyboardOpen: true, zoomed: false });
  });

  it('does not write scroll positions that are already at the origin', async () => {
    const { watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();

    viewport.height = 384;
    viewport.dispatchEvent(new Event('resize'));

    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
  });

  it('ignores a stale visual viewport while nothing takes typing', async () => {
    // iOS can keep a system sheet's keyboard subtracted from the visual
    // viewport after Safari returns from the background.
    const { visibleViewport, watchVisualViewport } = await importModule();
    fakeDocument.activeElement = { tagName: 'BODY' };
    viewport.height = 484;
    watchVisualViewport();
    flushFrames();

    expect(visibleViewport).toEqual({ top: 0, height: null, keyboardOpen: false, zoomed: false });
  });

  it('re-measures when focus leaves the text input', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();
    openKeyboardWithPageScroll();

    fakeDocument.activeElement = { tagName: 'BUTTON' };
    fakeDocument.dispatchEvent(new Event('focusout'));

    expect(visibleViewport).toEqual({ top: 0, height: null, keyboardOpen: false, zoomed: false });
  });

  it('a button input takes no typing', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    fakeDocument.activeElement = { tagName: 'INPUT', type: 'checkbox' };
    viewport.height = 484;
    watchVisualViewport();
    flushFrames();
    expect(visibleViewport.height).toBeNull();

    fakeDocument.activeElement = { tagName: 'INPUT', type: 'search' };
    fakeDocument.dispatchEvent(new Event('focusin'));
    expect(visibleViewport.height).toBe(484);
  });

  it('follows a focused contenteditable', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    fakeDocument.activeElement = { tagName: 'DIV', isContentEditable: true };
    viewport.height = 384;
    watchVisualViewport();
    flushFrames();

    expect(visibleViewport.keyboardOpen).toBe(true);
  });

  it('leaves a pinch-zoomed page where the user panned it', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();

    viewport.scale = 2;
    viewport.height = 357;
    viewport.offsetTop = 120;
    fakeWindow.scrollY = 120;
    viewport.dispatchEvent(new Event('scroll'));

    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
    expect(visibleViewport).toEqual({ top: 0, height: null, keyboardOpen: false, zoomed: true });
  });

  it('the last unsubscribe stops tracking and drops a stale keyboard', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    const unwatchA = watchVisualViewport();
    const unwatchB = watchVisualViewport();
    flushFrames();
    openKeyboardWithPageScroll();

    unwatchA();
    expect(visibleViewport.keyboardOpen).toBe(true);

    unwatchB();
    expect(visibleViewport).toEqual({ top: 0, height: null, keyboardOpen: false, zoomed: false });

    fakeWindow.scrollTo.mockClear();
    openKeyboardWithPageScroll();
    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
    expect(visibleViewport.height).toBeNull();
  });

  it('unsubscribing before the first frame cancels the read', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    const unwatch = watchVisualViewport();
    unwatch();
    flushFrames();

    expect(visibleViewport.height).toBeNull();
  });

  it('is a no-op without visualViewport', async () => {
    vi.stubGlobal('window', { scrollX: 0, scrollY: 0, scrollTo: vi.fn() });
    const { visibleViewport, watchVisualViewport } = await importModule();
    const unwatch = watchVisualViewport();
    flushFrames();
    unwatch();

    expect(visibleViewport).toEqual({ top: 0, height: null, keyboardOpen: false, zoomed: false });
  });
});
