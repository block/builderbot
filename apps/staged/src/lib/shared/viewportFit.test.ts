import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  SETTLE_MS,
  coverLaysOutShort,
  guardViewportFit,
  viewportFitChange,
  withViewportFitCover,
  withoutViewportFit,
} from './viewportFit';

// vitest runs in node: `window`, `document` and the safe-area probe are
// stubbed down to what the module touches. Numbers are the iPhone 17 Pro
// simulator's on iOS 27.2: an 874 pt window, an 812 pt layout viewport and a
// 62 pt status bar and a 34 pt home indicator inset.

const CONTENT =
  'width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, interactive-widget=resizes-content, viewport-fit=cover';

let standalone: boolean;
let safeAreaTop: number;
let safeAreaBottom: number;
let rootStyle: Map<string, string>;
let meta: { content: string };
let probe: { style: Record<string, string>; remove: ReturnType<typeof vi.fn> };
let fakeWindow: EventTarget & {
  outerWidth: number;
  outerHeight: number;
  matchMedia(query: string): unknown;
};
let documentElement: {
  clientHeight: number;
  appendChild: ReturnType<typeof vi.fn>;
  style: { setProperty(name: string, value: string): void; removeProperty(name: string): void };
};

function resize(): void {
  fakeWindow.dispatchEvent(new Event('resize'));
}

/** What iOS reports once it hands over the insets, with a `resize`. */
function insetsArrive(top: number): void {
  safeAreaTop = top;
  resize();
}

/** The reading has stood for the settle time, with no `resize`. */
function settle(): void {
  vi.advanceTimersByTime(SETTLE_MS);
}

/** iOS re-lays the page out after the meta changes: no insets without cover. */
function relayout(top: number, layoutHeight: number): void {
  safeAreaTop = top;
  safeAreaBottom = meta.content.includes('viewport-fit') ? 34 : 0;
  documentElement.clientHeight = layoutHeight;
  resize();
}

/** The window turns: 874 x 402 in landscape, where an iPhone has no status bar. */
function rotate(to: 'portrait' | 'landscape'): void {
  const portrait = to === 'portrait';
  fakeWindow.outerWidth = portrait ? 402 : 874;
  fakeWindow.outerHeight = portrait ? 874 : 402;
  const covered = meta.content.includes('viewport-fit');
  // Portrait as a black-translucent install has it: under the status bar and
  // short with cover, below the status bar without.
  relayout(portrait && covered ? 62 : 0, portrait ? 812 : 402);
}

beforeEach(() => {
  standalone = true;
  // Zero until iOS hands the insets over, a moment after launch.
  safeAreaTop = 0;
  safeAreaBottom = 34;
  rootStyle = new Map();
  vi.useFakeTimers();
  meta = { content: CONTENT };
  probe = { style: {}, remove: vi.fn() };
  fakeWindow = Object.assign(new EventTarget(), {
    outerWidth: 402,
    outerHeight: 874,
    matchMedia: (query: string) => ({
      matches: query === '(display-mode: standalone)' && standalone,
    }),
  });
  documentElement = {
    clientHeight: 812,
    appendChild: vi.fn(),
    style: {
      setProperty: (name, value) => void rootStyle.set(name, value),
      removeProperty: (name) => void rootStyle.delete(name),
    },
  };
  vi.stubGlobal('window', fakeWindow);
  vi.stubGlobal('navigator', {});
  vi.stubGlobal('document', {
    documentElement,
    createElement: () => probe,
    querySelector: (selector: string) => (selector === 'meta[name="viewport"]' ? meta : null),
  });
  vi.stubGlobal('getComputedStyle', () => ({
    paddingTop: `${safeAreaTop}px`,
    paddingBottom: `${safeAreaBottom}px`,
  }));
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('coverLaysOutShort', () => {
  const sample = { standalone: true, safeAreaTop: 62, layoutHeight: 812, windowHeight: 874 };

  it('is a page under the status bar that ends above the bottom of the window', () => {
    expect(coverLaysOutShort(sample)).toBe(true);
  });

  it('is not a page below the status bar (status bar default)', () => {
    expect(coverLaysOutShort({ ...sample, safeAreaTop: 0 })).toBe(false);
  });

  it('is not a page under the status bar that reaches the bottom', () => {
    expect(coverLaysOutShort({ ...sample, layoutHeight: 874 })).toBe(false);
  });

  it('is never Safari', () => {
    expect(coverLaysOutShort({ ...sample, standalone: false })).toBe(false);
  });
});

describe('viewportFitChange', () => {
  const short = { standalone: true, safeAreaTop: 62, layoutHeight: 812, windowHeight: 874 };

  it('drops cover where it lays the page out short', () => {
    expect(viewportFitChange(true, short)).toBe('drop');
    expect(viewportFitChange(true, { ...short, safeAreaTop: 0 })).toBeNull();
  });

  it('restores cover once the page reaches the bottom of the window without it', () => {
    const dropped = { ...short, safeAreaTop: 0 };
    expect(viewportFitChange(false, dropped)).toBeNull();
    expect(viewportFitChange(false, { ...dropped, layoutHeight: 874 })).toBe('restore');
  });

  it('asks for nothing in Safari', () => {
    expect(viewportFitChange(true, { ...short, standalone: false })).toBeNull();
    expect(viewportFitChange(false, { ...short, standalone: false, layoutHeight: 874 })).toBeNull();
  });
});

describe('withViewportFitCover', () => {
  it('puts back what withoutViewportFit took', () => {
    expect(withViewportFitCover(withoutViewportFit(CONTENT))).toBe(CONTENT);
    expect(withViewportFitCover(CONTENT)).toBe(CONTENT);
  });
});

describe('withoutViewportFit', () => {
  it('drops only the viewport-fit key', () => {
    expect(withoutViewportFit(CONTENT)).toBe(
      'width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, interactive-widget=resizes-content'
    );
    expect(withoutViewportFit('viewport-fit=cover,width=device-width')).toBe('width=device-width');
  });
});

describe('guardViewportFit', () => {
  it('drops cover once the insets show an app laid out short, and the reading settles', () => {
    guardViewportFit();
    insetsArrive(62);
    expect(meta.content).toBe(CONTENT);

    vi.advanceTimersByTime(SETTLE_MS - 1);
    expect(meta.content).toBe(CONTENT);
    vi.advanceTimersByTime(1);

    expect(meta.content).not.toContain('viewport-fit');
    expect(meta.content).toContain('width=device-width');
  });

  it('drops it after the settle time if the insets are already there', () => {
    safeAreaTop = 62;
    guardViewportFit();
    expect(meta.content).toBe(CONTENT);

    settle();

    expect(meta.content).not.toContain('viewport-fit');
  });

  it('ignores a single short reading that the next resize takes back', () => {
    guardViewportFit();
    insetsArrive(62);
    vi.advanceTimersByTime(SETTLE_MS - 1);
    insetsArrive(0);
    settle();
    expect(meta.content).toBe(CONTENT);
  });

  it('ignores a short reading that is gone by the time it would settle', () => {
    guardViewportFit();
    insetsArrive(62);
    // No `resize` announces the change.
    safeAreaTop = 0;
    settle();
    expect(meta.content).toBe(CONTENT);
  });

  it('counts the settle time from the last resize', () => {
    guardViewportFit();
    insetsArrive(62);
    vi.advanceTimersByTime(SETTLE_MS - 1);
    resize();
    vi.advanceTimersByTime(SETTLE_MS - 1);
    expect(meta.content).toBe(CONTENT);
    vi.advanceTimersByTime(1);
    expect(meta.content).not.toContain('viewport-fit');
  });

  it('keeps cover for an app added with the default status bar', () => {
    guardViewportFit();
    insetsArrive(0);
    settle();
    expect(meta.content).toBe(CONTENT);
    expect(rootStyle.size).toBe(0);
  });

  it('keeps cover where the page reaches the bottom of the window', () => {
    documentElement.clientHeight = 874;
    guardViewportFit();
    insetsArrive(59);
    settle();
    expect(meta.content).toBe(CONTENT);
  });

  it('keeps cover in Safari, without probing', () => {
    standalone = false;
    safeAreaTop = 62;
    guardViewportFit();
    settle();
    expect(meta.content).toBe(CONTENT);
    expect(documentElement.appendChild).not.toHaveBeenCalled();
  });

  it('leaves the bottom inset as a fallback when it drops cover', () => {
    guardViewportFit();
    insetsArrive(62);
    settle();
    expect(rootStyle.get('--safe-bottom-fallback')).toBe('34px');
  });

  it('sets no fallback where there was no bottom inset', () => {
    safeAreaBottom = 0;
    guardViewportFit();
    insetsArrive(62);
    settle();
    expect(meta.content).not.toContain('viewport-fit');
    expect(rootStyle.has('--safe-bottom-fallback')).toBe(false);
  });

  it('stays dropped while the page still ends above the bottom of the window', () => {
    guardViewportFit();
    insetsArrive(62);
    settle();

    relayout(0, 812);
    settle();

    expect(meta.content).not.toContain('viewport-fit');
    expect(rootStyle.get('--safe-bottom-fallback')).toBe('34px');
  });

  it('restores cover, and clears the fallback, once the page reaches the bottom', () => {
    guardViewportFit();
    insetsArrive(62);
    settle();
    relayout(0, 812);

    rotate('landscape');
    expect(meta.content).not.toContain('viewport-fit');
    settle();

    expect(meta.content).toBe(CONTENT);
    expect(rootStyle.has('--safe-bottom-fallback')).toBe(false);
  });

  it('does not restore on a reading that does not settle', () => {
    guardViewportFit();
    insetsArrive(62);
    settle();
    relayout(0, 812);

    relayout(0, 874);
    vi.advanceTimersByTime(SETTLE_MS - 1);
    relayout(0, 812);
    settle();

    expect(meta.content).not.toContain('viewport-fit');
    expect(rootStyle.get('--safe-bottom-fallback')).toBe('34px');
  });

  it('follows the window through rotations, dropping and restoring each time', () => {
    guardViewportFit();
    insetsArrive(62);
    settle();
    relayout(0, 812);

    for (let turn = 0; turn < 3; turn++) {
      rotate('landscape');
      settle();
      relayout(0, 402);
      expect(meta.content).toBe(CONTENT);

      rotate('portrait');
      settle();
      relayout(0, 812);
      expect(meta.content).not.toContain('viewport-fit');
      expect(rootStyle.get('--safe-bottom-fallback')).toBe('34px');
    }
  });

  it('stops flipping where cover lays out short and no cover reaches the bottom', () => {
    guardViewportFit();
    insetsArrive(62);
    settle();
    // Without cover the page fills the window, with it the page is short
    // again, and the window never changes.
    relayout(0, 874);
    settle();
    expect(meta.content).toBe(CONTENT);
    relayout(62, 812);
    settle();
    expect(meta.content).not.toContain('viewport-fit');

    relayout(0, 874);
    settle();
    expect(meta.content).not.toContain('viewport-fit');

    // A new window size is judged afresh.
    rotate('landscape');
    settle();
    expect(meta.content).toBe(CONTENT);
  });

  /** Cover came back in portrait and had to go again: it is kept off there. */
  function keptOffInPortrait(): void {
    guardViewportFit();
    insetsArrive(62);
    settle();
    relayout(0, 874);
    settle();
    relayout(62, 812);
    settle();
    expect(meta.content).not.toContain('viewport-fit');
  }

  it('keeps cover off at that size through a change of window where cover stays off', () => {
    keptOffInPortrait();

    // A window whose page ends above its bottom without cover: no restore.
    fakeWindow.outerWidth = 874;
    fakeWindow.outerHeight = 402;
    relayout(0, 380);
    settle();
    expect(meta.content).not.toContain('viewport-fit');

    // Back at the first size, where the page fills the window without cover.
    fakeWindow.outerWidth = 402;
    fakeWindow.outerHeight = 874;
    relayout(0, 874);
    settle();
    expect(meta.content).not.toContain('viewport-fit');
    expect(rootStyle.get('--safe-bottom-fallback')).toBe('34px');
  });

  it('tries cover once more at that size after it came back at another', () => {
    keptOffInPortrait();

    rotate('landscape');
    settle();
    expect(meta.content).toBe(CONTENT);

    // Short again with cover, so it goes; that drop lifts the limit.
    rotate('portrait');
    settle();
    expect(meta.content).not.toContain('viewport-fit');

    relayout(0, 874);
    settle();
    expect(meta.content).toBe(CONTENT);
    relayout(62, 812);
    settle();
    expect(meta.content).not.toContain('viewport-fit');

    // One restore and drop, then cover is kept off here as before.
    relayout(0, 874);
    settle();
    expect(meta.content).not.toContain('viewport-fit');
  });

  it('stops checking once cleaned up', () => {
    const stop = guardViewportFit();
    insetsArrive(62);
    stop();

    settle();
    insetsArrive(62);
    settle();

    expect(meta.content).toBe(CONTENT);
    expect(probe.remove).toHaveBeenCalled();
  });

  it('keeps its probe while it watches', () => {
    guardViewportFit();
    insetsArrive(62);
    settle();
    expect(probe.remove).not.toHaveBeenCalled();
  });
});
