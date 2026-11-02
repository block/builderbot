import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { coverLaysOutShort, guardViewportFit, withoutViewportFit } from './viewportFit';

// vitest runs in node: `window`, `document` and the safe-area probe are
// stubbed down to what the module touches. Numbers are the iPhone 17 Pro
// simulator's on iOS 27.2: an 874 pt window, an 812 pt layout viewport and a
// 62 pt status bar.

const CONTENT =
  'width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, interactive-widget=resizes-content, viewport-fit=cover';

let standalone: boolean;
let safeAreaTop: number;
let meta: { content: string };
let probe: { style: Record<string, string>; remove: ReturnType<typeof vi.fn> };
let fakeWindow: EventTarget & { outerHeight: number; matchMedia(query: string): unknown };
let documentElement: { clientHeight: number; appendChild: ReturnType<typeof vi.fn> };

/** What iOS reports once it hands over the insets, with a `resize`. */
function insetsArrive(top: number): void {
  safeAreaTop = top;
  fakeWindow.dispatchEvent(new Event('resize'));
}

beforeEach(() => {
  standalone = true;
  // Zero until iOS hands the insets over, a moment after launch.
  safeAreaTop = 0;
  meta = { content: CONTENT };
  probe = { style: {}, remove: vi.fn() };
  fakeWindow = Object.assign(new EventTarget(), {
    outerHeight: 874,
    matchMedia: (query: string) => ({
      matches: query === '(display-mode: standalone)' && standalone,
    }),
  });
  documentElement = { clientHeight: 812, appendChild: vi.fn() };
  vi.stubGlobal('window', fakeWindow);
  vi.stubGlobal('navigator', {});
  vi.stubGlobal('document', {
    documentElement,
    createElement: () => probe,
    querySelector: (selector: string) => (selector === 'meta[name="viewport"]' ? meta : null),
  });
  vi.stubGlobal('getComputedStyle', () => ({ paddingTop: `${safeAreaTop}px` }));
});

afterEach(() => {
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

describe('withoutViewportFit', () => {
  it('drops only the viewport-fit key', () => {
    expect(withoutViewportFit(CONTENT)).toBe(
      'width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no, interactive-widget=resizes-content'
    );
    expect(withoutViewportFit('viewport-fit=cover,width=device-width')).toBe('width=device-width');
  });
});

describe('guardViewportFit', () => {
  it('drops cover once the insets show an app laid out short', () => {
    guardViewportFit();
    expect(meta.content).toBe(CONTENT);

    insetsArrive(62);

    expect(meta.content).not.toContain('viewport-fit');
    expect(meta.content).toContain('width=device-width');
    expect(probe.remove).toHaveBeenCalled();
  });

  it('drops it at once if the insets are already there', () => {
    safeAreaTop = 62;
    guardViewportFit();
    expect(meta.content).not.toContain('viewport-fit');
  });

  it('keeps cover for an app added with the default status bar', () => {
    guardViewportFit();
    insetsArrive(0);
    expect(meta.content).toBe(CONTENT);
  });

  it('keeps cover where the page reaches the bottom of the window', () => {
    documentElement.clientHeight = 874;
    guardViewportFit();
    insetsArrive(59);
    expect(meta.content).toBe(CONTENT);
  });

  it('keeps cover in Safari, without probing', () => {
    standalone = false;
    safeAreaTop = 62;
    guardViewportFit();
    expect(meta.content).toBe(CONTENT);
    expect(documentElement.appendChild).not.toHaveBeenCalled();
  });

  it('stops checking once cleaned up', () => {
    const stop = guardViewportFit();
    stop();

    insetsArrive(62);

    expect(meta.content).toBe(CONTENT);
    expect(probe.remove).toHaveBeenCalled();
  });
});
