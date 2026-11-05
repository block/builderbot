import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createScrollIfActive } from './activeRowScroll';

let activeRow: HTMLElement | null;
let scrollRestored: boolean;

const scrollIfActive = createScrollIfActive({
  activeRow: () => activeRow,
  setActiveRow: (node) => {
    activeRow = node;
  },
  scrollRestored: () => scrollRestored,
});

function row() {
  return { scrollIntoView: vi.fn() } as unknown as HTMLElement & {
    scrollIntoView: ReturnType<typeof vi.fn>;
  };
}

beforeEach(() => {
  activeRow = null;
  scrollRestored = true;
});

describe('scrollIfActive', () => {
  it('scrolls a row that mounts active into view once the scroll is restored', () => {
    // The review's case: a filter shows [A1, A2*, A3]. Removing A2 opens B2,
    // which the filter rules out, so the sidebar appends B2's row as the
    // active one. It never sees an inactive → active update.
    const a2 = row();
    const a2Action = scrollIfActive(a2, true);
    const b2 = row();

    a2Action.destroy?.();
    scrollIfActive(b2, true);

    expect(b2.scrollIntoView).toHaveBeenCalledExactlyOnceWith({ block: 'nearest' });
    expect(activeRow).toBe(b2);
  });

  it('leaves a row that mounts active to the scroll restore before it has run', () => {
    scrollRestored = false;
    const node = row();

    scrollIfActive(node, true);

    expect(node.scrollIntoView).not.toHaveBeenCalled();
    // Recorded, so the restore can bring it into view itself.
    expect(activeRow).toBe(node);
  });

  it('does not scroll or record a row that mounts inactive', () => {
    const node = row();

    scrollIfActive(node, false);

    expect(node.scrollIntoView).not.toHaveBeenCalled();
    expect(activeRow).toBeNull();
  });

  it('scrolls a row into view as it becomes active, only once the scroll is restored', () => {
    scrollRestored = false;
    const node = row();
    const action = scrollIfActive(node, false);

    action.update?.(true);
    expect(node.scrollIntoView).not.toHaveBeenCalled();
    expect(activeRow).toBe(node);

    action.update?.(false);
    expect(activeRow).toBeNull();

    scrollRestored = true;
    action.update?.(true);
    expect(node.scrollIntoView).toHaveBeenCalledExactlyOnceWith({ block: 'nearest' });
  });

  it('keeps the newly active row when the previous one deactivates or unmounts after it', () => {
    const previous = row();
    const previousAction = scrollIfActive(previous, true);
    const next = row();
    const nextAction = scrollIfActive(next, false);

    nextAction.update?.(true);
    previousAction.update?.(false);
    expect(activeRow).toBe(next);

    previousAction.destroy?.();
    expect(activeRow).toBe(next);

    nextAction.destroy?.();
    expect(activeRow).toBeNull();
  });
});
