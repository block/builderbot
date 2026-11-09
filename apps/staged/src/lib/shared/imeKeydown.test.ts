import { describe, expect, it } from 'vitest';
import { isImeKeydown } from './imeKeydown';

describe('isImeKeydown', () => {
  it('is true during a composition', () => {
    expect(isImeKeydown({ isComposing: true, keyCode: 13 })).toBe(true);
  });

  it("is true for Safari's committing Enter, after compositionend", () => {
    expect(isImeKeydown({ isComposing: false, keyCode: 229 })).toBe(true);
  });

  it('is false for a plain key', () => {
    expect(isImeKeydown({ isComposing: false, keyCode: 13 })).toBe(false);
  });
});
