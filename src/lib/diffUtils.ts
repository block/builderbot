/**
 * Diff Utilities
 *
 * Pure helper functions for diff display.
 */

import type { FileDiff, Alignment } from './types';

/**
 * Get display path from a FileDiff, handling renames.
 */
export function getDisplayPath(diff: FileDiff): string {
  const beforePath = diff.before?.path ?? null;
  const afterPath = diff.after?.path ?? null;

  if (beforePath && afterPath && beforePath !== afterPath) {
    return `${beforePath} → ${afterPath}`;
  }
  return afterPath || beforePath || '';
}

/**
 * Get the primary path for a diff (prefers after, falls back to before).
 */
export function getFilePath(diff: FileDiff): string | null {
  return diff.after?.path ?? diff.before?.path ?? null;
}

/**
 * Check if a line is at the start or end of a changed alignment.
 * Used to draw horizontal separator lines in CSS.
 *
 * For empty spans (e.g., the "before" side of a pure insert), we draw a single
 * line to avoid the double-thick appearance from adjacent top/bottom borders.
 * - If there's a preceding line, draw on its bottom edge
 * - If at file start (no preceding line), draw on the following line's top edge
 */
export function getLineBoundary(
  alignments: Alignment[],
  side: 'before' | 'after',
  lineIndex: number
): { isStart: boolean; isEnd: boolean } {
  for (const alignment of alignments) {
    if (!alignment.changed) continue;

    const span = side === 'before' ? alignment.before : alignment.after;

    // Empty span: draw a single line at the insertion point.
    // Use alignment-start on the line AT span.start (its top edge aligns with
    // where the connector attaches at span.start * lineHeight).
    if (span.start === span.end) {
      if (lineIndex === span.start) {
        return { isStart: true, isEnd: false };
      }
      continue;
    }

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
  if (diff.after?.path) return detectLanguage(diff.after.path);
  if (diff.before?.path) return detectLanguage(diff.before.path);
  return null;
}

/**
 * Check if a diff represents a binary file.
 */
export function isBinaryDiff(diff: FileDiff): boolean {
  const beforeBinary = diff.before?.content.type === 'binary';
  const afterBinary = diff.after?.content.type === 'binary';
  return beforeBinary || afterBinary;
}

/**
 * Get text lines from a file, or empty array if binary/null.
 */
export function getTextLines(diff: FileDiff, side: 'before' | 'after'): string[] {
  const file = side === 'before' ? diff.before : diff.after;
  if (!file || file.content.type === 'binary') return [];
  return file.content.lines;
}
