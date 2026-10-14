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
  // Runes are not compiled under vitest; the controller only needs a plain value.
  beforeEach(() => {
    vi.stubGlobal('$state', (value: unknown) => value);
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
  });
});
