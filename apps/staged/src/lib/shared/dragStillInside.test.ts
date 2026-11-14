import { describe, expect, it } from 'vitest';
import { dragStillInside } from './dragStillInside';

describe('dragStillInside', () => {
  // Enough of three elements: a container at (10, 20) to (110, 70), a child
  // of it, and one outside it.
  const child = {} as Node;
  const outside = {} as Node;
  const container = {
    contains: (node: Node | null) => node === child || node === container,
    getBoundingClientRect: () => ({ left: 10, right: 110, top: 20, bottom: 70 }) as DOMRect,
  } as unknown as Element;

  it('goes by what the drag moved on to, when the event says', () => {
    // The pointer disagrees in each, and is not consulted.
    expect(dragStillInside(container, { clientX: 0, clientY: 0, relatedTarget: child })).toBe(true);
    expect(dragStillInside(container, { clientX: 0, clientY: 0, relatedTarget: container })).toBe(
      true
    );
    expect(dragStillInside(container, { clientX: 50, clientY: 50, relatedTarget: outside })).toBe(
      false
    );
  });

  it('goes by the pointer when the event names nothing', () => {
    const at = (clientX: number, clientY: number) => ({ clientX, clientY, relatedTarget: null });
    expect(dragStillInside(container, at(10, 20))).toBe(true);
    expect(dragStillInside(container, at(109, 69))).toBe(true);
    expect(dragStillInside(container, at(9, 40))).toBe(false);
    expect(dragStillInside(container, at(110, 40))).toBe(false);
    expect(dragStillInside(container, at(50, 19))).toBe(false);
    expect(dragStillInside(container, at(50, 70))).toBe(false);
  });
});
