// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createScrollController, type PaneDimensions } from '../state/scrollController.svelte';
import type { Alignment } from '../types';
import { setupDiffKeyboardNav } from './diffKeyboard';

const LINE_HEIGHT = 20;

function dims(lineCount: number, viewportRows: number): PaneDimensions {
  return {
    viewportHeight: viewportRows * LINE_HEIGHT,
    contentHeight: lineCount * LINE_HEIGHT,
    lineHeight: LINE_HEIGHT,
  };
}

function hunk(before: [number, number], after: [number, number], changed = true): Alignment {
  return {
    before: { start: before[0], end: before[1] },
    after: { start: after[0], end: after[1] },
    changed,
  };
}

/** Rows visible in a pane at the given scroll offset: [first, lastExclusive). */
function visibleRows(scrollY: number, viewportRows: number): [number, number] {
  const first = Math.floor(scrollY / LINE_HEIGHT);
  return [first, first + viewportRows];
}

function press(key: string) {
  window.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
}

/**
 * Wire keyboard navigation to a real scroll controller the way DiffViewer does,
 * including the host's focusedHunkIndex round-trip through onHunkFocus.
 */
function setup(opts: {
  beforeLines: number;
  afterLines: number;
  viewportRows: number;
  alignments: Alignment[];
}) {
  const controller = createScrollController();
  controller.setDimensions('before', dims(opts.beforeLines, opts.viewportRows));
  controller.setDimensions('after', dims(opts.afterLines, opts.viewportRows));
  controller.setAlignments(opts.alignments, 'file.ts');

  const changed = opts.alignments
    .map((alignment, index) => ({ alignment, index }))
    .filter(({ alignment }) => alignment.changed);

  const host = { focusedHunkIndex: null as number | null };
  const scrollToRow = vi.fn((row: number, side: 'before' | 'after') =>
    controller.scrollToRow(row, side)
  );
  const startCommentOnHunk = vi.fn();

  const cleanup = setupDiffKeyboardNav({
    getChangedAlignments: () => changed,
    scrollToRow,
    scrollBy: (deltaY) => controller.scrollBy('after', deltaY),
    getCurrentScrollY: () => controller.afterScrollY,
    getLineHeight: () => controller.getDimensions('after').lineHeight,
    getViewportHeight: () => controller.getDimensions('after').viewportHeight,
    startCommentOnHunk,
    onHunkFocus: (index) => {
      host.focusedHunkIndex = index;
    },
    getFocusedHunkIndex: () => host.focusedHunkIndex,
  });

  return { controller, host, scrollToRow, startCommentOnHunk, cleanup };
}

describe('diffKeyboard hunk navigation', () => {
  let cleanup: (() => void) | null = null;

  // Runes are not compiled under vitest; the controller only needs plain values.
  beforeEach(() => {
    const identity = (value: unknown) => value;
    vi.stubGlobal('$state', Object.assign(identity, { raw: identity }));
  });

  afterEach(() => {
    cleanup?.();
    cleanup = null;
    vi.unstubAllGlobals();
  });

  describe('40-line file in a 40-row pane: modification at line 30, lines 40-60 deleted', () => {
    const VIEWPORT_ROWS = 40;
    const BEFORE_LINES = 60;
    const AFTER_LINES = 40;
    const alignments: Alignment[] = [
      hunk([0, 30], [0, 30], false),
      hunk([30, 31], [30, 31]),
      hunk([31, 40], [31, 40], false),
      hunk([40, 60], [40, 40]),
    ];

    function setupTrailingDeletion() {
      const result = setup({
        beforeLines: BEFORE_LINES,
        afterLines: AFTER_LINES,
        viewportRows: VIEWPORT_ROWS,
        alignments,
      });
      cleanup = result.cleanup;
      return result;
    }

    it('j from the top lands on the modification, which the after pane cannot scroll to', () => {
      const { controller, host, scrollToRow } = setupTrailingDeletion();

      press('j');

      expect(scrollToRow).toHaveBeenLastCalledWith(30, 'after');
      expect(host.focusedHunkIndex).toBe(0);
      // The whole after file fits, so the pane is pinned at 0 and the anchor
      // row never reaches line 30.
      expect(controller.afterScrollY).toBe(0);
    });

    it('j again advances to the trailing deletion with the deleted rows in the before pane', () => {
      const { controller, host, scrollToRow } = setupTrailingDeletion();
      press('j');

      press('j');

      expect(scrollToRow).toHaveBeenLastCalledWith(40, 'before');
      expect(host.focusedHunkIndex).toBe(1);
      const [first, last] = visibleRows(controller.beforeScrollY, VIEWPORT_ROWS);
      expect(first).toBeLessThanOrEqual(40);
      expect(last).toBe(BEFORE_LINES);
      // The after pane has nothing below alignment to show; it stays put.
      expect(controller.afterScrollY).toBe(0);
    });

    it('j on the trailing deletion has nowhere to go and leaves the view alone', () => {
      const { controller, host, scrollToRow } = setupTrailingDeletion();
      press('j');
      press('j');
      const beforeScrollY = controller.beforeScrollY;
      scrollToRow.mockClear();

      press('j');

      expect(scrollToRow).not.toHaveBeenCalled();
      expect(host.focusedHunkIndex).toBe(1);
      expect(controller.beforeScrollY).toBe(beforeScrollY);
    });

    it('k from the trailing deletion returns to the modification', () => {
      const { controller, host, scrollToRow } = setupTrailingDeletion();
      press('j');
      press('j');

      press('k');

      expect(scrollToRow).toHaveBeenLastCalledWith(30, 'after');
      expect(host.focusedHunkIndex).toBe(0);
      expect(controller.beforeScrollY).toBe(0);
      expect(controller.afterScrollY).toBe(0);
    });

    it('falls back to the scroll position once the host clears the focused hunk', () => {
      const { host, scrollToRow } = setupTrailingDeletion();
      press('j');
      host.focusedHunkIndex = null; // e.g. the user clicked elsewhere

      press('j');

      // With the after pane pinned at 0 the scroll anchor sits above line 30,
      // so without a trusted focus the first hunk is the next one again.
      expect(scrollToRow).toHaveBeenLastCalledWith(30, 'after');
      expect(host.focusedHunkIndex).toBe(0);
    });

    it('i comments on the focused trailing deletion', () => {
      const { startCommentOnHunk } = setupTrailingDeletion();
      press('j');
      press('j');

      press('i');

      expect(startCommentOnHunk).toHaveBeenCalledWith(1);
    });
  });

  describe('mid-file hunks in a scrollable 200-line file', () => {
    const VIEWPORT_ROWS = 15;
    const LINES = 200;
    const alignments: Alignment[] = [
      hunk([0, 50], [0, 50], false),
      hunk([50, 51], [50, 51]),
      hunk([51, 100], [51, 100], false),
      hunk([100, 101], [100, 101]),
      hunk([101, 150], [101, 150], false),
      hunk([150, 151], [150, 151]),
      hunk([151, 200], [151, 200], false),
    ];

    /** Scroll offset that puts `row` at the 1/3 anchor, as scrollToRow does. */
    function anchoredScrollY(row: number): number {
      return row * LINE_HEIGHT - (VIEWPORT_ROWS * LINE_HEIGHT) / 3;
    }

    function setupMidFile() {
      const result = setup({
        beforeLines: LINES,
        afterLines: LINES,
        viewportRows: VIEWPORT_ROWS,
        alignments,
      });
      cleanup = result.cleanup;
      return result;
    }

    it('j steps through each hunk in order and k steps back', () => {
      const { controller, host } = setupMidFile();

      press('j');
      expect(controller.afterScrollY).toBe(anchoredScrollY(50));
      expect(host.focusedHunkIndex).toBe(0);

      press('j');
      expect(controller.afterScrollY).toBe(anchoredScrollY(100));
      expect(host.focusedHunkIndex).toBe(1);

      press('j');
      expect(controller.afterScrollY).toBe(anchoredScrollY(150));
      expect(host.focusedHunkIndex).toBe(2);

      press('k');
      expect(controller.afterScrollY).toBe(anchoredScrollY(100));
      expect(host.focusedHunkIndex).toBe(1);

      // Both panes stay level mid-file.
      expect(controller.beforeScrollY).toBe(controller.afterScrollY);
    });

    it('re-derives the current hunk from the scroll position after the user scrolls', () => {
      const { controller, host, scrollToRow } = setupMidFile();
      press('j');
      press('j');
      expect(host.focusedHunkIndex).toBe(1);

      // Wheel 10 rows down past the second hunk; focus is stale now.
      controller.scrollBy('after', 10 * LINE_HEIGHT);

      // k first snaps back to the start of the hunk we are inside of...
      press('k');
      expect(scrollToRow).toHaveBeenLastCalledWith(100, 'after');
      expect(controller.afterScrollY).toBe(anchoredScrollY(100));

      // ...and k again goes to the previous hunk.
      press('k');
      expect(scrollToRow).toHaveBeenLastCalledWith(50, 'after');
      expect(host.focusedHunkIndex).toBe(0);
    });

    it('j after a manual scroll advances from the hunk under the anchor', () => {
      const { controller, scrollToRow } = setupMidFile();
      press('j');
      press('j');
      controller.scrollBy('after', 10 * LINE_HEIGHT);

      press('j');

      expect(scrollToRow).toHaveBeenLastCalledWith(150, 'after');
      expect(controller.afterScrollY).toBe(anchoredScrollY(150));
    });

    it('k before any hunk jumps to the first one', () => {
      const { controller, host } = setupMidFile();

      press('k');

      expect(controller.afterScrollY).toBe(anchoredScrollY(50));
      expect(host.focusedHunkIndex).toBe(0);
    });
  });
});
