import { describe, expect, it } from 'vitest';
import type { Alignment } from '../types';
import { anchorSideForHunk } from './diffViewerHelpers';

function hunk(before: [number, number], after: [number, number]): Alignment {
  return {
    before: { start: before[0], end: before[1] },
    after: { start: after[0], end: after[1] },
    changed: true,
  };
}

describe('anchorSideForHunk', () => {
  it('anchors a pure deletion on the before pane, where its rows exist', () => {
    expect(anchorSideForHunk(hunk([80, 100], [80, 80]))).toBe('before');
    expect(anchorSideForHunk(hunk([0, 20], [0, 0]))).toBe('before');
  });

  it('anchors modifications and additions on the after pane', () => {
    expect(anchorSideForHunk(hunk([10, 12], [10, 15]))).toBe('after');
    expect(anchorSideForHunk(hunk([10, 10], [10, 15]))).toBe('after');
  });
});
