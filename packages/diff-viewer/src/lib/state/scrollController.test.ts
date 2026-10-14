import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Alignment } from '../types';
import { createScrollController, type PaneDimensions } from './scrollController.svelte';

const LINE_HEIGHT = 20;
/** 15 rows tall; the 1/3 scroll anchor lands exactly on a row boundary. */
const VIEWPORT_HEIGHT = 300;

function dims(lineCount: number): PaneDimensions {
  return {
    viewportHeight: VIEWPORT_HEIGHT,
    contentHeight: lineCount * LINE_HEIGHT,
    lineHeight: LINE_HEIGHT,
  };
}

function maxScroll(lineCount: number): number {
  return lineCount * LINE_HEIGHT - VIEWPORT_HEIGHT;
}

/** Last row index (exclusive) that fits in a pane's viewport at the given offset. */
function lastVisibleRowEnd(scrollY: number): number {
  return Math.floor((scrollY + VIEWPORT_HEIGHT) / LINE_HEIGHT);
}

function setup(beforeLines: number, afterLines: number, alignments: Alignment[]) {
  const controller = createScrollController();
  controller.setDimensions('before', dims(beforeLines));
  controller.setDimensions('after', dims(afterLines));
  controller.setAlignments(alignments, 'file.ts');
  return controller;
}

describe('scrollController edge handoff', () => {
  // Runes are not compiled under vitest; the controller only needs plain values.
  beforeEach(() => {
    const identity = (value: unknown) => value;
    vi.stubGlobal('$state', Object.assign(identity, { raw: identity }));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  describe('final hunk deletes the last 20 of 100 lines', () => {
    const BEFORE_LINES = 100;
    const AFTER_LINES = 80;
    const alignments: Alignment[] = [
      { before: { start: 0, end: 80 }, after: { start: 0, end: 80 }, changed: false },
      { before: { start: 80, end: 100 }, after: { start: 80, end: 80 }, changed: true },
    ];

    it('leaves the deleted tail just past the before viewport when the after pane bottoms out', () => {
      const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);

      controller.scrollBy('after', maxScroll(AFTER_LINES));

      expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));
      // Shared rows stay level, so the before pane ends exactly where the deletion starts.
      expect(lastVisibleRowEnd(controller.beforeScrollY)).toBe(80);
    });

    it('hands further after-pane scrolling to the before pane so every deleted line is reachable', () => {
      const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);
      controller.scrollBy('after', maxScroll(AFTER_LINES));

      controller.scrollBy('after', 20 * LINE_HEIGHT);

      expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));
      expect(controller.beforeScrollY).toBe(maxScroll(BEFORE_LINES));
      expect(lastVisibleRowEnd(controller.beforeScrollY)).toBe(BEFORE_LINES);
    });

    it('unwinds the handoff before the after pane moves back up', () => {
      const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);
      controller.scrollBy('after', maxScroll(AFTER_LINES));
      controller.scrollBy('after', 20 * LINE_HEIGHT);

      controller.scrollBy('after', -5 * LINE_HEIGHT);
      expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));
      expect(controller.beforeScrollY).toBe(maxScroll(BEFORE_LINES) - 5 * LINE_HEIGHT);

      // 15 rows unwind the remaining handoff, the other 20 move both panes together.
      controller.scrollBy('after', -35 * LINE_HEIGHT);
      expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES) - 20 * LINE_HEIGHT);
      expect(controller.beforeScrollY).toBe(controller.afterScrollY);
    });

    it('keeps the panes alignment-synced away from the edges', () => {
      const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);

      controller.scrollBy('after', 20 * LINE_HEIGHT);
      expect(controller.beforeScrollY).toBe(20 * LINE_HEIGHT);

      controller.scrollBy('after', -5 * LINE_HEIGHT);
      expect(controller.beforeScrollY).toBe(15 * LINE_HEIGHT);
    });

    it('unwinds a handoff reached from the before pane before the after pane moves', () => {
      const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);

      // Scrolling the before pane to its end clamps the after pane at its own max,
      // leaving the same 20-row handoff that wheeling the after pane would create.
      controller.scrollBy('before', 5000);
      expect(controller.beforeScrollY).toBe(maxScroll(BEFORE_LINES));
      expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));

      controller.scrollBy('after', -5 * LINE_HEIGHT);
      expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));
      expect(controller.beforeScrollY).toBe(maxScroll(BEFORE_LINES) - 5 * LINE_HEIGHT);

      // 15 more rows finish unwinding, the last 5 move both panes together.
      controller.scrollBy('after', -20 * LINE_HEIGHT);
      expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES) - 5 * LINE_HEIGHT);
      expect(controller.beforeScrollY).toBe(controller.afterScrollY);
    });

    it('snaps away an outstanding handoff on programmatic navigation', () => {
      const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);
      controller.scrollBy('after', maxScroll(AFTER_LINES) + 20 * LINE_HEIGHT);
      expect(controller.beforeScrollY).toBe(maxScroll(BEFORE_LINES));

      controller.scrollTo('after', maxScroll(AFTER_LINES));
      expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));
      expect(controller.beforeScrollY).toBe(maxScroll(AFTER_LINES));

      controller.scrollBy('after', 20 * LINE_HEIGHT);
      controller.scrollToRow(60, 'after');
      expect(controller.afterScrollY).toBe(60 * LINE_HEIGHT - VIEWPORT_HEIGHT / 3);
      expect(controller.beforeScrollY).toBe(controller.afterScrollY);
    });

    describe('effective extent (scrollbar range)', () => {
      it('extends the after pane by the deleted tail and leaves the before pane alone', () => {
        const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);

        expect(controller.getEffectiveExtent('after')).toEqual({
          scrollY: 0,
          maxScrollY: maxScroll(BEFORE_LINES),
          contentHeight: BEFORE_LINES * LINE_HEIGHT,
          contentOffset: 0,
        });
        expect(controller.getEffectiveExtent('before')).toEqual({
          scrollY: 0,
          maxScrollY: maxScroll(BEFORE_LINES),
          contentHeight: BEFORE_LINES * LINE_HEIGHT,
          contentOffset: 0,
        });
      });

      it('keeps the position moving through a wheel handoff', () => {
        const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);

        controller.scrollBy('after', maxScroll(AFTER_LINES));
        expect(controller.getEffectiveExtent('after').scrollY).toBe(maxScroll(AFTER_LINES));

        controller.scrollBy('after', 20 * LINE_HEIGHT);
        expect(controller.getEffectiveExtent('after').scrollY).toBe(maxScroll(BEFORE_LINES));
      });

      it('scrollToEffective carries the before pane through the tail with the after pane pinned', () => {
        const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);

        controller.scrollToEffective('after', maxScroll(BEFORE_LINES));
        expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));
        expect(controller.beforeScrollY).toBe(maxScroll(BEFORE_LINES));

        // Dragging back up unwinds the handoff; the position reports exactly what was requested.
        const partway = maxScroll(AFTER_LINES) + 5 * LINE_HEIGHT;
        controller.scrollToEffective('after', partway);
        expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));
        expect(controller.beforeScrollY).toBe(partway);
        expect(controller.getEffectiveExtent('after').scrollY).toBe(partway);

        // Inside the pane's own range it behaves like scrollTo.
        controller.scrollToEffective('after', 40 * LINE_HEIGHT);
        expect(controller.afterScrollY).toBe(40 * LINE_HEIGHT);
        expect(controller.beforeScrollY).toBe(40 * LINE_HEIGHT);
      });

      it('clamps scrollToEffective to the extent', () => {
        const controller = setup(BEFORE_LINES, AFTER_LINES, alignments);

        controller.scrollToEffective('after', 99999);
        expect(controller.afterScrollY).toBe(maxScroll(AFTER_LINES));
        expect(controller.beforeScrollY).toBe(maxScroll(BEFORE_LINES));

        controller.scrollToEffective('after', -50);
        expect(controller.afterScrollY).toBe(0);
        expect(controller.beforeScrollY).toBe(0);
      });
    });
  });

  it('mirrors the handoff for lines appended to the end of a file', () => {
    const controller = setup(80, 100, [
      { before: { start: 0, end: 80 }, after: { start: 0, end: 80 }, changed: false },
      { before: { start: 80, end: 80 }, after: { start: 80, end: 100 }, changed: true },
    ]);

    controller.scrollBy('before', 5000);

    expect(controller.beforeScrollY).toBe(maxScroll(80));
    expect(controller.afterScrollY).toBe(maxScroll(100));
  });

  it('mirrors the effective extent for lines appended to the end of a file', () => {
    const controller = setup(80, 100, [
      { before: { start: 0, end: 80 }, after: { start: 0, end: 80 }, changed: false },
      { before: { start: 80, end: 80 }, after: { start: 80, end: 100 }, changed: true },
    ]);

    expect(controller.getEffectiveExtent('before').maxScrollY).toBe(maxScroll(100));
    expect(controller.getEffectiveExtent('after').maxScrollY).toBe(maxScroll(100));

    controller.scrollToEffective('before', maxScroll(100));
    expect(controller.beforeScrollY).toBe(maxScroll(80));
    expect(controller.afterScrollY).toBe(maxScroll(100));
  });

  describe('first hunk deletes the first 20 of 100 lines', () => {
    const alignments: Alignment[] = [
      { before: { start: 0, end: 20 }, after: { start: 0, end: 0 }, changed: true },
      { before: { start: 20, end: 100 }, after: { start: 0, end: 80 }, changed: false },
    ];

    it('hands off at the top so the deleted head is reachable after explicit navigation', () => {
      const controller = setup(100, 80, alignments);

      // Navigation lands on aligned positions: after row 0 is level with before row 20.
      controller.scrollToRow(0, 'after');
      expect(controller.afterScrollY).toBe(0);
      expect(controller.beforeScrollY).toBe(20 * LINE_HEIGHT);

      controller.scrollBy('after', -20 * LINE_HEIGHT);
      expect(controller.afterScrollY).toBe(0);
      expect(controller.beforeScrollY).toBe(0);
    });

    it('brings the before pane into alignment before leaving the top, and the round trip returns there', () => {
      const controller = setup(100, 80, alignments);

      controller.scrollBy('after', 25 * LINE_HEIGHT);
      expect(controller.afterScrollY).toBe(5 * LINE_HEIGHT);
      expect(controller.beforeScrollY).toBe(25 * LINE_HEIGHT);

      controller.scrollBy('after', -25 * LINE_HEIGHT);
      expect(controller.afterScrollY).toBe(0);
      expect(controller.beforeScrollY).toBe(0);
    });

    it('exposes the deleted head as top headroom of the after pane', () => {
      const controller = setup(100, 80, alignments);

      controller.scrollToRow(0, 'after');
      const extent = controller.getEffectiveExtent('after');
      expect(extent.contentOffset).toBe(20 * LINE_HEIGHT);
      expect(extent.maxScrollY).toBe(maxScroll(100));
      // Aligned at after row 0 the thumb sits below the headroom, not at the very top.
      expect(extent.scrollY).toBe(20 * LINE_HEIGHT);

      controller.scrollToEffective('after', 0);
      expect(controller.afterScrollY).toBe(0);
      expect(controller.beforeScrollY).toBe(0);
      expect(controller.getEffectiveExtent('after').scrollY).toBe(0);

      controller.scrollToEffective('after', 30 * LINE_HEIGHT);
      expect(controller.afterScrollY).toBe(10 * LINE_HEIGHT);
      expect(controller.beforeScrollY).toBe(30 * LINE_HEIGHT);
    });
  });
});
