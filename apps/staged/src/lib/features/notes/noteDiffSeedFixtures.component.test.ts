/**
 * The note-diff seeder (`src-tauri/src/note_versions_seed.rs`) leaves a note
 * whose last revision is meant to show one of each change. The fixtures are
 * markdown and the changes are read from rendered blocks, so this renders
 * the last two revisions as the viewer does and holds them to that list.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';

import previousDraft from '../../../../src-tauri/src/note_versions_seed/revision-3.md?raw';
import currentDraft from '../../../../src-tauri/src/note_versions_seed/revision-4.md?raw';
import type { PikchrRenderer } from '../../shared/markdown/pikchrRendering';
import { computeNoteDiff, type NoteDiffHunk } from './noteDiff';
import { extractNoteDiffUnits, type NoteDiffDomUnit } from './noteDiffUnits';
import { renderNoteMarkdown } from './noteMarkdown';

/** Stands in for the WASM renderer: a drawing that differs with its source. */
const pikchrRenderer: PikchrRenderer = (source) => ({
  kind: 'svg',
  svg:
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 40">' +
    `<text x="10" y="20">${source.replace(/[^a-z ]/gi, '')}</text></svg>`,
  width: 100,
  height: 40,
});

/** A draft is the note as the viewer renders it: the title heading, then the body. */
function units(draft: string): NoteDiffDomUnit[] {
  const html = renderNoteMarkdown(draft, { pikchrRenderer });
  const parsed = new DOMParser().parseFromString(`<div>${html}</div>`, 'text/html');
  return extractNoteDiffUnits(parsed.body.firstElementChild as HTMLElement);
}

beforeEach(() => {
  vi.spyOn(console, 'info').mockImplementation(() => {});
});

describe('the note-diff seed fixtures', () => {
  it('change the note in each of the ways the change bar marks, one hunk apiece', () => {
    const previous = units(previousDraft);
    const current = units(currentDraft);
    const kindsOf = (all: NoteDiffDomUnit[], indices: number[]) =>
      indices.map((index) => all[index].kind);
    const describeHunk = (hunk: NoteDiffHunk) => ({
      kind: hunk.kind,
      previous: kindsOf(previous, hunk.previousIndices),
      current: kindsOf(current, hunk.currentIndices),
    });

    const { hunks } = computeNoteDiff(previous, current);

    expect(hunks.map(describeHunk)).toEqual([
      // A reworded sentence in the summary.
      { kind: 'modified', previous: ['p'], current: ['p'] },
      // An edited diagram.
      { kind: 'modified', previous: ['diagram'], current: ['diagram'] },
      // A removed paragraph.
      { kind: 'removed', previous: ['p'], current: [] },
      // A changed list item.
      { kind: 'modified', previous: ['ul-li'], current: ['ul-li'] },
      // A changed table row.
      { kind: 'modified', previous: ['tr'], current: ['tr'] },
      // An added section: its heading, a paragraph and three steps.
      { kind: 'added', previous: [], current: ['h2', 'p', 'ol-li', 'ol-li', 'ol-li'] },
    ]);
    // The reworded sentence and the changed cell are narrow enough to be
    // pointed at by word, not marked whole.
    for (const index of [0, 4]) {
      const [op] = hunks[index].ops;
      expect(op.type === 'modified' && !op.wholeUnit && op.currentRanges.length > 0).toBe(true);
    }
  });

  it('keep the note’s title, so the first block is unchanged', () => {
    expect(units(currentDraft)[0]).toMatchObject(units(previousDraft)[0]);
    expect(units(currentDraft)[0].kind).toBe('h1');
  });
});
