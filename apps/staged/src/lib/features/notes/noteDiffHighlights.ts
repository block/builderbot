/**
 * Showing a note diff in a rendered note, without adding to its markup.
 *
 * The note is `{@html}` markup that two other features already rewrite: a
 * reply draft's quotes are wrapped in spans and find-in-note's matches in
 * marks, both redone on every render. A third set of wrappers would have to
 * be woven through theirs. So a change is shown two ways that leave the
 * nodes alone:
 *
 * - Blocks carry attributes, for a stylesheet to tint: `data-note-diff` (how
 *   the block itself changed, which is its colour), `data-note-diff-hunk`
 *   (which hunk it is in) and `data-note-diff-active` (it is the hunk being
 *   pointed at). The markup is rebuilt on every render, so they are put
 *   back then.
 * - Words are painted through the CSS Custom Highlight API: ranges
 *   registered under a name and styled with `::highlight()`. The ranges are
 *   built when a hunk is pointed at, from the text as it then stands: a
 *   range kept from before a quote was wrapped would end in the wrong node.
 *   A runtime without the API shows the block tint alone.
 *
 * `pointAtNoteDiffHunk` does both for the hunk being pointed at, and is what
 * the viewer's two panes call.
 */

import {
  hunkTextRanges,
  type NoteDiffChange,
  type NoteDiffHunk,
  type NoteDiffSide,
  type NoteDiffTextRange,
} from './noteDiff';
import { readNoteDiffUnitText, type NoteDiffDomUnit } from './noteDiffUnits';

/**
 * On a changed block: how it changed. `modified` when the other version has
 * its counterpart, and otherwise `added` (in the note) or `removed` (in the
 * previous version).
 *
 * This is the block's change and not its hunk's, as the code diff viewer
 * colours each line by its own class: in a hunk the change bar marks as
 * modified, a paragraph the revision replaced outright is removed on one
 * side and added on the other.
 */
export const NOTE_DIFF_ATTRIBUTE = 'data-note-diff';
/** On a changed block: its hunk's id. */
export const NOTE_DIFF_HUNK_ATTRIBUTE = 'data-note-diff-hunk';
/** On the blocks of the hunk being pointed at. */
export const NOTE_DIFF_ACTIVE_ATTRIBUTE = 'data-note-diff-active';

/** The highlight over the words a changed block gained, in the note. */
export const NOTE_DIFF_ADDED_HIGHLIGHT = 'note-diff-added';
/** The highlight over the words a changed block lost, in the previous version. */
export const NOTE_DIFF_REMOVED_HIGHLIGHT = 'note-diff-removed';

const MARKED = `[${NOTE_DIFF_ATTRIBUTE}], [${NOTE_DIFF_HUNK_ATTRIBUTE}], [${NOTE_DIFF_ACTIVE_ATTRIBUTE}]`;

/** Take every diff attribute off the blocks under `root`. */
export function clearNoteDiffMarks(root: Element): void {
  for (const element of root.querySelectorAll(MARKED)) {
    element.removeAttribute(NOTE_DIFF_ATTRIBUTE);
    element.removeAttribute(NOTE_DIFF_HUNK_ATTRIBUTE);
    element.removeAttribute(NOTE_DIFF_ACTIVE_ATTRIBUTE);
  }
}

/**
 * Mark the blocks of every hunk on one side, replacing whatever marks
 * `root` held. `units` are that side's, read from `root`.
 *
 * A hunk with no block on this side (text only the other version has) marks
 * nothing here; `hunkExtent` says where its gap is.
 */
export function applyNoteDiffMarks(
  root: Element,
  units: readonly NoteDiffDomUnit[],
  hunks: readonly NoteDiffHunk[],
  side: NoteDiffSide
): void {
  clearNoteDiffMarks(root);
  for (const hunk of hunks) {
    for (const op of hunk.ops) {
      const index = unitIndexOn(op, side);
      const element = index === null ? undefined : units[index]?.element;
      if (!element) continue;
      element.setAttribute(NOTE_DIFF_ATTRIBUTE, op.type);
      element.setAttribute(NOTE_DIFF_HUNK_ATTRIBUTE, hunk.id);
    }
  }
}

/** The block a change has on one side, or `null` where only the other version has one. */
function unitIndexOn(op: NoteDiffChange, side: NoteDiffSide): number | null {
  if (side === 'previous') return op.type === 'added' ? null : op.previousIndex;
  return op.type === 'removed' ? null : op.currentIndex;
}

/** Make `hunkId`'s blocks under `root` the active ones, or none with `null`. */
export function setActiveNoteDiffHunk(root: Element, hunkId: string | null): void {
  for (const element of root.querySelectorAll(`[${NOTE_DIFF_HUNK_ATTRIBUTE}]`)) {
    const active = hunkId !== null && element.getAttribute(NOTE_DIFF_HUNK_ATTRIBUTE) === hunkId;
    if (active) element.setAttribute(NOTE_DIFF_ACTIVE_ATTRIBUTE, '');
    else element.removeAttribute(NOTE_DIFF_ACTIVE_ATTRIBUTE);
  }
}

/**
 * Stretches of a unit's text as DOM ranges, or its whole text for `null`.
 *
 * Nothing is returned for a unit whose text is no longer what the diff
 * compared: the note was rendered again, and the offsets are another text's.
 */
function unitRanges(unit: NoteDiffDomUnit, ranges: NoteDiffTextRange[] | null): Range[] {
  const read = readNoteDiffUnitText(unit.element, true);
  if (read.text !== unit.text) return [];

  const result: Range[] = [];
  for (const { start, end } of ranges ?? [{ start: 0, end: read.text.length }]) {
    // The ends are moved in past characters no text node holds.
    let first = Math.max(0, start);
    let last = Math.min(end, read.text.length) - 1;
    while (first <= last && read.nodeOf[first] === -1) first++;
    while (last >= first && read.nodeOf[last] === -1) last--;
    if (first > last) continue;
    const range = unit.element.ownerDocument.createRange();
    range.setStart(read.nodes[read.nodeOf[first]], read.offsetOf[first]);
    range.setEnd(read.nodes[read.nodeOf[last]], read.offsetOf[last] + 1);
    result.push(range);
  }
  return result;
}

/**
 * The text to paint for a hunk on one side (see `hunkTextRanges` for which
 * text that is). `units` are that side's.
 */
export function noteDiffHunkRanges(
  units: readonly NoteDiffDomUnit[],
  hunk: NoteDiffHunk,
  side: NoteDiffSide
): Range[] {
  return hunkTextRanges(hunk, side).flatMap(({ unitIndex, ranges }) => {
    const unit = units[unitIndex];
    return unit ? unitRanges(unit, ranges) : [];
  });
}

function highlightName(side: NoteDiffSide): string {
  return side === 'previous' ? NOTE_DIFF_REMOVED_HIGHLIGHT : NOTE_DIFF_ADDED_HIGHLIGHT;
}

/**
 * Paint `ranges` as one side's changed words, in place of what was painted
 * for it. Each side has a highlight of its own, since each pane paints its
 * own ranges; the stylesheet gives the two one colour. No ranges clears the
 * side. Returns whether the runtime can paint at all.
 */
export function paintNoteDiffWords(side: NoteDiffSide, ranges: readonly Range[]): boolean {
  const registry = globalThis.CSS?.highlights;
  if (!registry || typeof Highlight === 'undefined') return false;
  if (ranges.length === 0) registry.delete(highlightName(side));
  else registry.set(highlightName(side), new Highlight(...ranges));
  return true;
}

/**
 * Show `hunk` as the one being pointed at on one side: its blocks active
 * and its changed words painted. `units` are that side's, read from `root`.
 * `null` points at nothing. Returns the undo.
 *
 * The words stay painted as the note's text nodes are rewritten under them:
 * a quote wrapped in its spans, find-in-note's marks put in or taken out.
 * A range made before such a pass ends in a node that has since been split
 * or moved, so the ranges are made again after each one. Attributes are not
 * watched, so the marks set here start nothing.
 */
export function pointAtNoteDiffHunk(
  root: Element,
  units: readonly NoteDiffDomUnit[],
  hunk: NoteDiffHunk | null,
  side: NoteDiffSide
): () => void {
  setActiveNoteDiffHunk(root, hunk?.id ?? null);
  if (!hunk) return () => {};

  const paint = () => paintNoteDiffWords(side, noteDiffHunkRanges(units, hunk, side));
  const observer =
    paint() && typeof MutationObserver !== 'undefined' ? new MutationObserver(paint) : null;
  observer?.observe(root, { childList: true, subtree: true, characterData: true });
  return () => {
    observer?.disconnect();
    paintNoteDiffWords(side, []);
    setActiveNoteDiffHunk(root, null);
  };
}
