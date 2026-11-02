import { describe, expect, it } from 'vitest';
import { safeAreaOffset } from './safeArea';

describe('safeAreaOffset', () => {
  it('adds each edge’s own safe-area inset to the distance', () => {
    expect(safeAreaOffset('16px')).toEqual({
      top: 'calc(16px + env(safe-area-inset-top))',
      right: 'calc(16px + env(safe-area-inset-right))',
      bottom: 'calc(16px + env(safe-area-inset-bottom))',
      left: 'calc(16px + env(safe-area-inset-left))',
    });
  });
});
