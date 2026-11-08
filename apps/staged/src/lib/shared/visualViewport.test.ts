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

/** A stand-in element; `inFullScreen` places it inside a full-screen dialog. */
function element(props: Record<string, unknown>, inFullScreen = true) {
  return {
    ...props,
    closest: (selector: string) => (selector === '[data-full-screen]' && inFullScreen ? {} : null),
  };
}

const composer = element({ tagName: 'TEXTAREA' });

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
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** Fake only the timers the blur fallback uses; rAF stays the stub above. */
function useFakeTimeouts(): void {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
}

/** What iOS does when a tap takes focus off the input: no viewport change yet. */
function blurTo(target: unknown): void {
  fakeDocument.activeElement = { tagName: 'BODY' };
  fakeDocument.dispatchEvent(new Event('focusout'));
  fakeDocument.activeElement = target;
  if (target && (target as { tagName?: string }).tagName !== 'BODY') {
    fakeDocument.dispatchEvent(new Event('focusin'));
  }
}

/** The keyboard leaving: the visual viewport grows back to the layout viewport. */
function closeKeyboard(): void {
  viewport.height = 714;
  viewport.offsetTop = 0;
  viewport.dispatchEvent(new Event('resize'));
}

const KEYBOARD_GEOMETRY = { top: 0, height: 384, keyboardOpen: true, zoomed: false };
const LAYOUT_GEOMETRY = { top: 0, height: null, keyboardOpen: false, zoomed: false };

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

  it('holds the keyboard geometry after a blur until the keyboard leaves', async () => {
    // Tapping Send or Start: iOS takes focus off the input on the tap's
    // mousedown and delivers the click afterwards at the same point. The
    // layout must not move in between, or the click misses the button.
    useFakeTimeouts();
    const { visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();
    openKeyboardWithPageScroll();
    expect(visibleViewport).toEqual(KEYBOARD_GEOMETRY);

    blurTo({ tagName: 'BODY' });
    expect(visibleViewport).toEqual(KEYBOARD_GEOMETRY);

    closeKeyboard();
    expect(visibleViewport).toEqual(LAYOUT_GEOMETRY);
    // The resize settled the blur; the fallback has nothing left to do.
    expect(vi.getTimerCount()).toBe(0);
  });

  it('treats focus landing on something that takes no typing as a blur', async () => {
    // A tap inside the dialog that focuses the dialog itself (tabindex=-1)
    // or a button fires `focusin` right after the input's `focusout`.
    useFakeTimeouts();
    const { visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();
    openKeyboardWithPageScroll();

    blurTo(element({ tagName: 'DIV', role: 'dialog' }));
    expect(visibleViewport).toEqual(KEYBOARD_GEOMETRY);
    blurTo(element({ tagName: 'BUTTON' }));
    expect(visibleViewport).toEqual(KEYBOARD_GEOMETRY);

    closeKeyboard();
    expect(visibleViewport).toEqual(LAYOUT_GEOMETRY);
  });

  it('re-measures a blur after BLUR_SETTLE_MS when no resize comes', async () => {
    // A hardware keyboard, or a blur with no keyboard on screen at all.
    useFakeTimeouts();
    const { BLUR_SETTLE_MS, visibleViewport, watchVisualViewport } = await importModule();
    fakeDocument.activeElement = element({ tagName: 'INPUT', type: 'text' });
    watchVisualViewport();
    flushFrames();
    expect(visibleViewport).toEqual({ top: 0, height: 714, keyboardOpen: false, zoomed: false });

    blurTo({ tagName: 'BODY' });
    // A second focus change while waiting does not push the deadline back.
    vi.advanceTimersByTime(BLUR_SETTLE_MS - 100);
    blurTo(element({ tagName: 'BUTTON' }));
    vi.advanceTimersByTime(99);
    expect(visibleViewport.height).toBe(714);

    vi.advanceTimersByTime(1);
    expect(visibleViewport).toEqual(LAYOUT_GEOMETRY);
  });

  it('moving focus to another text input re-measures at once and drops the wait', async () => {
    useFakeTimeouts();
    const { BLUR_SETTLE_MS, visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();
    openKeyboardWithPageScroll();

    const search = element({ tagName: 'INPUT', type: 'search' });
    blurTo(search);
    expect(vi.getTimerCount()).toBe(0);
    expect(visibleViewport).toEqual(KEYBOARD_GEOMETRY);

    // The keyboard stays up for the new input; nothing resets it later.
    vi.advanceTimersByTime(BLUR_SETTLE_MS * 2);
    expect(visibleViewport).toEqual(KEYBOARD_GEOMETRY);
  });

  it('keeps undoing the page scroll during a pending blur without moving the layout', async () => {
    useFakeTimeouts();
    const { visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();
    openKeyboardWithPageScroll();
    blurTo({ tagName: 'BODY' });
    fakeWindow.scrollTo.mockClear();

    fakeWindow.scrollY = 40;
    viewport.offsetTop = 40;
    viewport.dispatchEvent(new Event('scroll'));

    expect(fakeWindow.scrollTo).toHaveBeenCalledWith(0, 0);
    expect(visibleViewport).toEqual(KEYBOARD_GEOMETRY);
  });

  it('the last unsubscribe cancels a pending blur', async () => {
    useFakeTimeouts();
    const { visibleViewport, watchVisualViewport } = await importModule();
    const unwatch = watchVisualViewport();
    flushFrames();
    openKeyboardWithPageScroll();
    blurTo({ tagName: 'BODY' });
    expect(vi.getTimerCount()).toBe(1);

    unwatch();
    expect(vi.getTimerCount()).toBe(0);
    expect(visibleViewport).toEqual(LAYOUT_GEOMETRY);
  });

  it('a button input takes no typing', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    fakeDocument.activeElement = { tagName: 'INPUT', type: 'checkbox' };
    viewport.height = 484;
    watchVisualViewport();
    flushFrames();
    expect(visibleViewport.height).toBeNull();

    fakeDocument.activeElement = element({ tagName: 'INPUT', type: 'search' });
    fakeDocument.dispatchEvent(new Event('focusin'));
    expect(visibleViewport.height).toBe(484);
  });

  it('follows a focused contenteditable', async () => {
    const { visibleViewport, watchVisualViewport } = await importModule();
    fakeDocument.activeElement = element({ tagName: 'DIV', isContentEditable: true });
    viewport.height = 384;
    watchVisualViewport();
    flushFrames();

    expect(visibleViewport.keyboardOpen).toBe(true);
  });

  it('keeps the page scroll that reveals an input outside the full-screen layout', async () => {
    // A popover's search field portalled over the dialog. guardDocumentScroll
    // keeps Safari's scroll for it, so the layout stays at the layout
    // viewport and scrolls with the page, popover and all, rather than
    // following the visual viewport back over the input.
    const { visibleViewport, watchVisualViewport } = await importModule();
    watchVisualViewport();
    flushFrames();

    fakeDocument.activeElement = element({ tagName: 'INPUT', type: 'text' }, false);
    viewport.height = 384;
    viewport.offsetTop = 330;
    fakeWindow.scrollY = 330;
    viewport.dispatchEvent(new Event('resize'));

    expect(fakeWindow.scrollTo).not.toHaveBeenCalled();
    expect(fakeWindow.scrollY).toBe(330);
    expect(visibleViewport).toEqual({ top: 0, height: null, keyboardOpen: false, zoomed: false });

    // Back in the dialog's own composer, the dialog fits above the keyboard
    // again and the scroll goes.
    fakeDocument.activeElement = composer;
    fakeDocument.dispatchEvent(new Event('focusin'));

    expect(fakeWindow.scrollTo).toHaveBeenCalledWith(0, 0);
    expect(visibleViewport).toEqual({ top: 0, height: 384, keyboardOpen: true, zoomed: false });
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
