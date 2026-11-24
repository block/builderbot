import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { NoteVersion } from '../../types';

const getNotePreviousVersion = vi.hoisted(() =>
  vi.fn<(noteId: string) => Promise<NoteVersion | null>>()
);
vi.mock('../../api/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/commands')>()),
  getNotePreviousVersion,
}));

import { NoteDiffState, parseNoteDiffUnits } from './noteDiffState.svelte';
import { extractNoteDiffUnits } from './noteDiffUnits';
import { renderNoteMarkdown } from './noteMarkdown';
import { CURRENT_NOTE, HUNK_KINDS, PREVIOUS_NOTE } from './testStubs/noteDiffFixtures';

/**
 * The note viewer's diff state: which previous version it holds for which
 * text, when it compares, and what selecting a change does.
 */

const version = (id: string, content = 'Before.'): NoteVersion => ({
  id,
  noteId: 'note-1',
  title: 'Cache design',
  content,
  savedAt: 1_000,
});
const subject = (content: string) => ({ id: 'note-1', title: 'Cache design', content });

/** A fetch the test answers when it chooses. */
function deferred() {
  let resolve!: (version: NoteVersion | null) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<NoteVersion | null>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

function unitsOf(markdown: string) {
  const root = document.createElement('div');
  root.innerHTML = renderNoteMarkdown(markdown);
  return extractNoteDiffUnits(root);
}

/** A state holding the diff of the two fixture versions. */
function compared() {
  const state = new NoteDiffState();
  state.compare(renderNoteMarkdown(PREVIOUS_NOTE), unitsOf(CURRENT_NOTE));
  return state;
}

let info: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  getNotePreviousVersion.mockReset();
  // `computeLineDiff` reports each run, which is how a comparison is counted.
  info = vi.spyOn(console, 'info').mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('NoteDiffState.load', () => {
  it('holds the previous version the store returns for the note', async () => {
    getNotePreviousVersion.mockResolvedValue(version('v1'));
    const state = new NoteDiffState();

    state.load(subject('Now.'));
    expect(state.previous).toBeNull();
    await settle();

    expect(getNotePreviousVersion).toHaveBeenCalledExactlyOnceWith('note-1');
    expect(state.previous).toEqual(version('v1'));
  });

  it('holds none for a note that has not been revised', async () => {
    getNotePreviousVersion.mockResolvedValue(null);
    const state = new NoteDiffState();

    state.load(subject('Now.'));
    await settle();

    expect(state.previous).toBeNull();
  });

  it('drops the version at once when the note’s text is another, and takes the new one', async () => {
    getNotePreviousVersion.mockResolvedValueOnce(version('v1'));
    const state = new NoteDiffState();
    state.load(subject('Second.'));
    await settle();

    const next = deferred();
    getNotePreviousVersion.mockReturnValueOnce(next.promise);
    state.load(subject('Third.'));
    // Not compared with text it does not precede while the fetch is out.
    expect(state.previous).toBeNull();

    next.resolve(version('v2', 'Second.'));
    await settle();
    expect(state.previous?.id).toBe('v2');
  });

  it('keeps the version it holds through a fetch for the same text', async () => {
    const first = version('v1');
    getNotePreviousVersion.mockResolvedValueOnce(first);
    const state = new NoteDiffState();
    state.load(subject('Now.'));
    await settle();

    const again = deferred();
    getNotePreviousVersion.mockReturnValueOnce(again.promise);
    state.load(subject('Now.'));
    expect(state.previous).toBe(first);

    // The same version in a new object: a write that left the text alone.
    again.resolve(version('v1'));
    await settle();
    expect(state.previous).toBe(first);
  });

  it('ignores a fetch that a later load has superseded', async () => {
    const slow = deferred();
    getNotePreviousVersion.mockReturnValueOnce(slow.promise);
    getNotePreviousVersion.mockResolvedValueOnce(version('v2'));
    const state = new NoteDiffState();

    state.load(subject('Second.'));
    state.load(subject('Third.'));
    await settle();
    slow.resolve(version('v1'));
    await settle();

    expect(state.previous?.id).toBe('v2');
  });

  it('lets go of the version with no note to show', async () => {
    getNotePreviousVersion.mockResolvedValue(version('v1'));
    const state = new NoteDiffState();
    state.load(subject('Now.'));
    await settle();

    state.load(null);

    expect(state.previous).toBeNull();
    expect(getNotePreviousVersion).toHaveBeenCalledOnce();
  });

  it('shows no diff when the fetch fails', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    getNotePreviousVersion.mockRejectedValue(new Error('offline'));
    const state = new NoteDiffState();

    state.load(subject('Now.'));
    await settle();

    expect(state.previous).toBeNull();
    expect(warn).toHaveBeenCalledOnce();
  });
});

describe('NoteDiffState.compare', () => {
  it('reads the previous version’s blocks from its HTML as the viewer reads the note’s', () => {
    const html = renderNoteMarkdown(PREVIOUS_NOTE);

    expect(parseNoteDiffUnits(html)).toEqual(
      unitsOf(PREVIOUS_NOTE).map(({ kind, text, signature }) => ({ kind, text, signature }))
    );
  });

  it('computes the diff of the two versions', () => {
    const state = compared();

    expect(state.hunks.map((hunk) => hunk.kind)).toEqual(HUNK_KINDS);
    expect(state.previousUnits).toHaveLength(6);
  });

  it('has no diff until both versions are there', () => {
    const state = new NoteDiffState();

    state.compare(null, unitsOf(CURRENT_NOTE));
    expect(state.diff).toBeNull();
    state.compare(renderNoteMarkdown(PREVIOUS_NOTE), null);
    expect(state.diff).toBeNull();
    expect(info).not.toHaveBeenCalled();
  });

  it('keeps the diff it has for the same blocks, however they were rendered', () => {
    const state = compared();
    const diff = state.diff;
    const runs = info.mock.calls.length;
    state.select(state.hunks[1].id, true);

    // New elements, and HTML that differs only in what no block reads.
    state.compare(`${renderNoteMarkdown(PREVIOUS_NOTE)}\n`, unitsOf(CURRENT_NOTE));

    expect(state.diff).toBe(diff);
    expect(info.mock.calls.length).toBe(runs);
    expect(state.activeHunkId).toBe(diff!.hunks[1].id);
    expect(state.open).toBe(true);
  });

  it('lets go of a hovered change when it closes, as of the selected one', () => {
    const state = compared();
    state.select(state.hunks[1].id, true);
    // The pointer is on something the previous version's pane holds, which
    // goes with the pane: nothing will report it left.
    state.hover(state.hunks[0].id);

    state.close();

    expect(state.hoveredHunkId).toBeNull();
    expect(state.pointedHunkId).toBeNull();
    expect(state.open).toBe(false);
    expect(state.engaged).toBe(false);
  });

  it('points at nothing once the diff is another', () => {
    const state = compared();
    state.hover(state.hunks[0].id);
    state.select(state.hunks[1].id, true);

    state.compare(renderNoteMarkdown(PREVIOUS_NOTE), unitsOf(`${CURRENT_NOTE}\n\nOne more.`));

    expect(state.hunks).toHaveLength(3);
    expect(state.hoveredHunkId).toBeNull();
    expect(state.activeHunkId).toBeNull();
    // The previous version is still wanted beside the note.
    expect(state.open).toBe(true);
  });

  it('closes the previous version when there is nothing left to show', () => {
    const state = compared();
    state.select(state.hunks[1].id, true);

    state.compare(renderNoteMarkdown(CURRENT_NOTE), unitsOf(CURRENT_NOTE));
    expect(state.hunks).toEqual([]);
    expect(state.open).toBe(false);

    state.compare(null, unitsOf(CURRENT_NOTE));
    expect(state.diff).toBeNull();
    expect(state.engaged).toBe(false);
  });
});

describe('NoteDiffState pointing', () => {
  it('shows the hovered change over the selected one', () => {
    const state = compared();
    const [removed, modified] = state.hunks;

    state.select(modified.id, true);
    expect(state.pointedHunk).toBe(modified);

    state.hover(removed.id);
    expect(state.pointedHunk).toBe(removed);
    expect(state.activeHunk).toBe(modified);

    state.hover(null);
    expect(state.pointedHunk).toBe(modified);
  });

  it('opens the previous version on a selection where there is room for it', () => {
    const state = compared();
    const requests = state.alignRequest;

    state.select(state.hunks[0].id, true);

    expect(state.open).toBe(true);
    expect(state.engaged).toBe(true);
    expect(state.following).toBe(true);
    expect(state.alignRequest).toBe(requests + 1);

    // The same marker again scrolls back to the change.
    state.select(state.hunks[0].id, true);
    expect(state.activeHunkId).toBe(state.hunks[0].id);
    expect(state.alignRequest).toBe(requests + 2);
  });

  it('keeps a change lit without a second pane, and lets go on the same marker again', () => {
    const state = compared();

    state.select(state.hunks[0].id, false);
    expect(state.open).toBe(false);
    expect(state.activeHunkId).toBe(state.hunks[0].id);
    expect(state.engaged).toBe(true);

    state.select(state.hunks[2].id, false);
    expect(state.activeHunkId).toBe(state.hunks[2].id);

    state.select(state.hunks[2].id, false);
    expect(state.activeHunkId).toBeNull();
    expect(state.engaged).toBe(false);
  });

  it('ignores a hunk that is not in the diff', () => {
    const state = compared();

    state.select('hunk-99', true);

    expect(state.engaged).toBe(false);
  });

  it('closes the previous version and lets go of the selection together', () => {
    const state = compared();
    state.select(state.hunks[1].id, true);

    state.close();

    expect(state.open).toBe(false);
    expect(state.activeHunkId).toBeNull();
    expect(state.following).toBe(false);
  });

  it('stops following the selection once the reader scrolls a pane', () => {
    const state = compared();
    const scroller = document.createElement('div');
    const unwatch = state.watchReaderScroll(scroller);
    state.select(state.hunks[1].id, true);

    scroller.dispatchEvent(new Event('wheel'));
    expect(state.following).toBe(false);

    unwatch();
    state.select(state.hunks[1].id, true);
    scroller.dispatchEvent(new Event('wheel'));
    expect(state.following).toBe(true);
  });

  it('keeps the boxes it was given until they differ', () => {
    const state = compared();
    const boxes = [{ top: 0, height: 10 }];
    state.setBoxes('current', boxes);
    const held = state.boxes;

    state.setBoxes('current', [{ top: 0, height: 10 }]);
    expect(state.boxes).toBe(held);

    state.setBoxes('current', [{ top: 0, height: 12 }]);
    expect(state.boxes.current).toEqual([{ top: 0, height: 12 }]);
    expect(state.boxes.previous).toEqual([]);
  });
});
