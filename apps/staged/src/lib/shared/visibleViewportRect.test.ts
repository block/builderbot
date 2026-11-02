import { afterEach, describe, expect, it, vi } from 'vitest';
import { visibleViewportRect } from './visibleViewportRect';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('visibleViewportRect', () => {
  it('is the visual viewport, offset into the layout viewport', () => {
    vi.stubGlobal('window', {
      innerWidth: 402,
      innerHeight: 714,
      visualViewport: { offsetTop: 330, offsetLeft: 0, width: 402, height: 384 },
    });
    expect(visibleViewportRect()).toEqual({ top: 330, right: 402, bottom: 714, left: 0 });
  });

  it('falls back to the window without visualViewport', () => {
    vi.stubGlobal('window', { innerWidth: 1600, innerHeight: 1200 });
    expect(visibleViewportRect()).toEqual({ top: 0, right: 1600, bottom: 1200, left: 0 });
  });
});
