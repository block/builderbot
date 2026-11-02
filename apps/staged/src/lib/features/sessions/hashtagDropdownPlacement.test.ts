import { describe, expect, it } from 'vitest';
import { placeHashtagDropdown } from './hashtagDropdownPlacement';

// The iOS 27.2 home-screen app on an iPhone 17 Pro: 402 x 812, of which 416 px
// stay visible above the keyboard.
const full = { top: 0, right: 402, bottom: 812, left: 0 };
const aboveKeyboard = { top: 0, right: 402, bottom: 416, left: 0 };
const desktop = { top: 0, right: 1600, bottom: 1000, left: 0 };
const size = { width: 420, maxHeight: 280 };

describe('placeHashtagDropdown', () => {
  it('opens below the caret when the list fits there', () => {
    expect(placeHashtagDropdown({ top: 100, bottom: 120, left: 300 }, desktop, size)).toEqual({
      side: 'below',
      top: 124,
      left: 300,
      width: 420,
      maxHeight: 280,
    });
  });

  it('opens above a caret near the keyboard, measured from the visible bottom', () => {
    // Measured to `innerHeight`, 458 px looked free below this caret, so the
    // list opened under the keyboard.
    expect(
      placeHashtagDropdown({ top: 334, bottom: 354, left: 37 }, aboveKeyboard, size)
    ).toMatchObject({ side: 'above', top: 330, maxHeight: 280 });
  });

  it('caps the list to the room on the side it opens', () => {
    expect(
      placeHashtagDropdown({ top: 200, bottom: 220, left: 37 }, aboveKeyboard, size)
    ).toMatchObject({ side: 'above', top: 196, maxHeight: 188 });
  });

  it('keeps a usable minimum when neither side has room', () => {
    const short = { top: 0, right: 402, bottom: 120, left: 0 };
    expect(placeHashtagDropdown({ top: 40, bottom: 60, left: 37 }, short, size)).toMatchObject({
      side: 'below',
      top: 64,
      maxHeight: 96,
    });
  });

  it('measures from the visible top when the page is panned', () => {
    const panned = { top: 300, right: 402, bottom: 716, left: 0 };
    expect(placeHashtagDropdown({ top: 640, bottom: 660, left: 37 }, panned, size)).toMatchObject({
      side: 'above',
      maxHeight: 280,
    });
  });

  it('narrows to the screen and keeps clear of its right edge', () => {
    // Starting at a caret mid-line, the 420 px list ran off a phone screen.
    expect(placeHashtagDropdown({ top: 100, bottom: 120, left: 200 }, full, size)).toMatchObject({
      left: 16,
      width: 370,
    });
  });

  it('shifts left only as far as the window edge needs', () => {
    expect(
      placeHashtagDropdown({ top: 100, bottom: 120, left: 1400 }, desktop, size)
    ).toMatchObject({ left: 1164, width: 420 });
  });

  it('stays inside a pinch-zoomed viewport', () => {
    const zoomed = { top: 200, right: 301, bottom: 557, left: 100 };
    expect(placeHashtagDropdown({ top: 300, bottom: 320, left: 50 }, zoomed, size)).toMatchObject({
      left: 100,
      width: 169,
    });
  });
});
