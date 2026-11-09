import { describe, expect, it } from 'vitest';
import { computeViewportGeometry, LAYOUT_VIEWPORT_GEOMETRY } from './viewportGeometry';

// Samples are iPhone 17 Pro / iOS 27.2 Safari readings: a 714 px layout
// viewport, and a 384 px visual viewport once the keyboard is up.

const typing = { textInputFocused: true };

describe('computeViewportGeometry', () => {
  it('keeps the layout viewport for a focused input with no keyboard over it', () => {
    // A dialog's input focused programmatically: iOS shows no keyboard.
    expect(
      computeViewportGeometry({ height: 714, offsetTop: 0, scale: 1, layoutHeight: 714, ...typing })
    ).toEqual(LAYOUT_VIEWPORT_GEOMETRY);
    // Sub-pixel differences are rounding, not something covering the page.
    expect(
      computeViewportGeometry({
        height: 713.5,
        offsetTop: 0,
        scale: 1,
        layoutHeight: 714,
        ...typing,
      })
    ).toEqual(LAYOUT_VIEWPORT_GEOMETRY);
  });

  it('follows the visual viewport down when Safari has scrolled the page for the keyboard', () => {
    expect(
      computeViewportGeometry({
        height: 384,
        offsetTop: 330,
        scale: 1,
        layoutHeight: 714,
        ...typing,
      })
    ).toEqual({ top: 330, height: 384, keyboardOpen: true, zoomed: false });
  });

  it('stays at the top once that scroll is undone', () => {
    expect(
      computeViewportGeometry({ height: 384, offsetTop: 0, scale: 1, layoutHeight: 714, ...typing })
    ).toEqual({ top: 0, height: 384, keyboardOpen: true, zoomed: false });
  });

  it('does not read a small inset (accessory bar, toolbar resize) as a keyboard', () => {
    const geometry = computeViewportGeometry({
      height: 664,
      offsetTop: 0,
      scale: 1,
      layoutHeight: 714,
      textInputFocused: true,
    });
    expect(geometry.keyboardOpen).toBe(false);
    expect(geometry.height).toBe(664);
  });

  it('a visual viewport taller than the layout viewport is not a keyboard', () => {
    expect(
      computeViewportGeometry({ height: 754, offsetTop: 0, scale: 1, layoutHeight: 714, ...typing })
    ).toEqual(LAYOUT_VIEWPORT_GEOMETRY);
  });

  it('falls back to the layout viewport while pinch-zoomed', () => {
    expect(
      computeViewportGeometry({
        height: 357,
        offsetTop: 120,
        scale: 2,
        layoutHeight: 714,
        ...typing,
      })
    ).toEqual({ ...LAYOUT_VIEWPORT_GEOMETRY, zoomed: true });
  });

  it('ignores the visual viewport while nothing takes typing', () => {
    // iOS 27.2 after Safari returned from the background with a system
    // sheet's keyboard still subtracted: 484 px visible, nothing covering.
    expect(
      computeViewportGeometry({
        height: 484,
        offsetTop: 0,
        scale: 1,
        layoutHeight: 714,
        textInputFocused: false,
      })
    ).toEqual(LAYOUT_VIEWPORT_GEOMETRY);
  });

  it('never places the box above the layout viewport', () => {
    expect(
      computeViewportGeometry({
        height: 714,
        offsetTop: -12,
        scale: 1,
        layoutHeight: 714,
        ...typing,
      }).top
    ).toBe(0);
  });
});
