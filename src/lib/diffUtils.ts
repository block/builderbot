/**
 * Diff Utilities
 *
 * Pure helper functions for diff display.
 */

import type { FileDiff, Range } from './types';

/**
 * Get display path, handling renames.
 */
export function getDisplayPath(diff: FileDiff): string {
  const { before, after } = diff;

  if (before.path && after.path && before.path !== after.path) {
    return `${before.path} → ${after.path}`;
  }
  return after.path || before.path || '';
}

/**
 * Check if a line is at the start or end of a changed range.
 * Used to draw horizontal separator lines in CSS.
 */
export function getLineBoundary(
  ranges: Range[],
  side: 'before' | 'after',
  lineIndex: number
): { isStart: boolean; isEnd: boolean } {
  for (const range of ranges) {
    if (!range.changed) continue;

    const span = side === 'before' ? range.before : range.after;

    if (lineIndex === span.start) {
      return { isStart: true, isEnd: lineIndex === span.end - 1 };
    }
    if (lineIndex === span.end - 1) {
      return { isStart: false, isEnd: true };
    }
  }
  return { isStart: false, isEnd: false };
}

/**
 * Detect language from diff paths (prefers after path).
 */
export function getLanguageFromDiff<T>(
  diff: FileDiff,
  detectLanguage: (path: string) => T | null
): T | null {
  if (diff.after.path) return detectLanguage(diff.after.path);
  if (diff.before.path) return detectLanguage(diff.before.path);
  return null;
}
