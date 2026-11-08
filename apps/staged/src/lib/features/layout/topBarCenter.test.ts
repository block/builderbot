import { describe, expect, it } from 'vitest';
import { centerSlotRoom } from './topBarCenter';

describe('centerSlotRoom', () => {
  it('keeps clear of whichever side reaches nearer the centre', () => {
    // An 800px bar: the left side ends at 180, the right actions start at 560.
    // The centre is 400, so the right side (160 away) limits the slot.
    expect(
      centerSlotRoom({ barLeft: 0, barRight: 800, leftEnd: 180, rightStart: 560, gap: 8 })
    ).toBe(304);
    expect(
      centerSlotRoom({ barLeft: 0, barRight: 800, leftEnd: 300, rightStart: 760, gap: 8 })
    ).toBe(184);
  });

  it('measures from the bar, wherever it starts', () => {
    // The bar beside a 240px sidebar.
    expect(
      centerSlotRoom({ barLeft: 240, barRight: 1040, leftEnd: 420, rightStart: 800, gap: 8 })
    ).toBe(304);
  });

  it('is 0 once the sides reach past the centre', () => {
    expect(
      centerSlotRoom({ barLeft: 0, barRight: 600, leftEnd: 320, rightStart: 400, gap: 8 })
    ).toBe(0);
  });
});
