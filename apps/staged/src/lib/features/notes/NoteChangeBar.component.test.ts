import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import NoteChangeBar from './NoteChangeBar.svelte';
import blocksSource from './NoteDiffBlocks.svelte?raw';
import markersSource from './NoteDiffMarkers.svelte?raw';
import {
  NOTE_DIFF_ACTIVE_ATTRIBUTE,
  NOTE_DIFF_ADDED_HIGHLIGHT,
  NOTE_DIFF_ATTRIBUTE,
} from './noteDiffHighlights';
import { BLOCK_BLEED, GAP_MARKER_HEIGHT } from './noteDiffScroll';
import { NoteDiffState } from './noteDiffState.svelte';
import {
  BLOCK_HEIGHT,
  CURRENT_NOTE,
  FakeHighlight,
  fakeResizeObserver,
  HUNK_KINDS,
  layOut,
  paintedText,
  pointerArrives,
  PREVIOUS_NOTE,
} from './testStubs/noteDiffFixtures';
import { renderNoteMarkdown } from './noteMarkdown';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

/**
 * The change bar beside a note: one marker per hunk of the diff against the
 * previous version, level with its change and coloured, lighting it on
 * hover. The note here is plain markup in a scroller, laid out by stubbed
 * offsets, and the bar is mounted in the scroller as the viewer mounts it.
 */

let component: ReturnType<typeof mount> | null = null;
let highlights: Map<string, FakeHighlight>;
let resize: () => void;

beforeEach(() => {
  document.body.innerHTML = '';
  highlights = new Map();
  vi.stubGlobal('CSS', { highlights });
  vi.stubGlobal('Highlight', FakeHighlight);
  const observer = fakeResizeObserver();
  vi.stubGlobal('ResizeObserver', observer.FakeResizeObserver);
  resize = observer.resize;
  // `computeLineDiff` reports each run.
  vi.spyOn(console, 'info').mockImplementation(() => {});
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function render(previous: string | null, current = CURRENT_NOTE, viewport = 300, margin = 0) {
  const scrollEl = document.createElement('div');
  const contentEl = document.createElement('div');
  contentEl.innerHTML = renderNoteMarkdown(current);
  scrollEl.append(contentEl);
  document.body.append(scrollEl);
  layOut(contentEl, scrollEl, viewport, BLOCK_HEIGHT, margin);

  const noteDiff = new NoteDiffState();
  const onSelect = vi.fn<(hunkId: string) => void>();
  const props = reactiveProps({
    noteDiff,
    contentEl,
    scrollEl,
    renderKey: 0,
    previousHtml: previous === null ? null : renderNoteMarkdown(previous),
    showAll: false,
    onSelect,
  });
  component = mount(NoteChangeBar, { target: scrollEl, props });
  flushSync();
  return { props, noteDiff, contentEl, scrollEl, onSelect };
}

const bar = () => document.querySelector<HTMLElement>('.note-change-bar');
const markers = () => [...document.querySelectorAll<HTMLButtonElement>('.note-change-marker')];
const marker = (kind: string) => markers().find((el) => el.dataset.kind === kind)!;
const layer = () => document.querySelector<HTMLElement>('.note-diff-blocks');
const blocks = () => [...document.querySelectorAll<HTMLElement>('.note-diff-block')];
const block = (kind: string) => blocks().find((el) => el.dataset.kind === kind)!;
const gaps = () => [...document.querySelectorAll<HTMLElement>('.note-diff-gap')];
/** A rectangle's parts from the top down: the kind and the height of each. */
const parts = (el: Element) =>
  [...el.querySelectorAll<HTMLElement>('.note-diff-block-part')].map((part) => [
    part.dataset.kind,
    part.style.height,
  ]);
const paragraph = (el: Element, text: string) =>
  [...el.querySelectorAll('p')].find((p) => p.textContent === text)!;
const marked = (el: Element) =>
  [...el.querySelectorAll(`[${NOTE_DIFF_ATTRIBUTE}]`)].map((block) => [
    block.textContent,
    block.getAttribute(NOTE_DIFF_ATTRIBUTE),
  ]);
const active = (el: Element) =>
  [...el.querySelectorAll(`[${NOTE_DIFF_ACTIVE_ATTRIBUTE}]`)].map((block) => block.textContent);

function hover(el: Element) {
  pointerArrives(el);
  flushSync();
}

/** A pointer event at a place in the window; a move bubbles there, as the browser's does. */
function pointer(type: string, el: Element, x: number, y: number) {
  el.dispatchEvent(
    new MouseEvent(type, { clientX: x, clientY: y, bubbles: type !== 'pointerenter' })
  );
  flushSync();
}

const TWO_ADDED = [CURRENT_NOTE, 'They are chosen by last use.'].join('\n\n');
/** A section added at the end: a heading, a sentence and a list of three, which are five blocks. */
const SECTION_ADDED = [
  CURRENT_NOTE.replace('\n\nA hundred entries go at a time.', ''),
  '## Rollout',
  'It ships in three steps.',
  '- Staff first.\n- Then a tenth of users.\n- Then everyone.',
].join('\n\n');
const SECTION_PREVIOUS = PREVIOUS_NOTE.replace('warmed', 'filled').replace(
  '\n\nLogs are kept for a week.',
  ''
);
/** The reworded paragraph, and one added straight after it: one change, of two kinds of block. */
const REWORDED_THEN_ADDED = CURRENT_NOTE.replace(
  'The cache is filled when the app starts.',
  'The cache is filled when the app starts.\n\nIt is filled from the last snapshot.'
).replace('\n\nA hundred entries go at a time.', '');
const REWORDED_PREVIOUS = PREVIOUS_NOTE.replace('\n\nLogs are kept for a week.', '');
/** The gap `layOut` leaves between blocks in the tests of what a margin does. */
const MARGIN = 20;

function leave(el: Element) {
  el.dispatchEvent(new MouseEvent('pointerleave'));
  flushSync();
}

describe('NoteChangeBar', () => {
  it('renders nothing for a note with no previous version', () => {
    const { contentEl, noteDiff } = render(null);

    expect(bar()).toBeNull();
    expect(noteDiff.diff).toBeNull();
    expect(marked(contentEl)).toEqual([]);
  });

  it('renders nothing when the previous version reads the same', () => {
    const { contentEl, noteDiff } = render(CURRENT_NOTE);

    expect(noteDiff.diff?.hunks).toEqual([]);
    expect(bar()).toBeNull();
    expect(marked(contentEl)).toEqual([]);
  });

  it('shows one marker per hunk, coloured by its kind and named for a screen reader', () => {
    const { contentEl } = render(PREVIOUS_NOTE);

    expect(markers().map((el) => el.dataset.kind)).toEqual(HUNK_KINDS);
    expect(markers().map((el) => el.getAttribute('aria-label'))).toEqual([
      'Removed: 1 block',
      'Modified: 1 block',
      'Added: 1 block',
    ]);
    expect(markers().map((el) => el.getAttribute('aria-pressed'))).toEqual([
      'false',
      'false',
      'false',
    ]);
    // The changed blocks carry their hunk's kind; nothing is lit yet.
    expect(marked(contentEl)).toEqual([
      ['The cache is filled when the app starts.', 'modified'],
      ['A hundred entries go at a time.', 'added'],
    ]);
    expect(active(contentEl)).toEqual([]);
  });

  it('stands in the element that scrolls the note, so the markers scroll with it', () => {
    const { scrollEl } = render(PREVIOUS_NOTE);

    expect(bar()!.parentElement).toBe(scrollEl);
  });

  it('stands each marker level with its change and as tall as its block', () => {
    // Six blocks of 100px in a 300px viewport: the note scrolls, and a
    // marker is where its block is in the note all the same.
    render(PREVIOUS_NOTE);

    expect(marker('modified').style.top).toBe(`${3 * BLOCK_HEIGHT}px`);
    expect(marker('modified').style.height).toBe(`${BLOCK_HEIGHT}px`);
    // The note's last block: the rectangle behind it, which the marker ends,
    // reaches a little past it where nothing follows.
    expect(marker('added').style.top).toBe(`${5 * BLOCK_HEIGHT}px`);
    expect(marker('added').style.height).toBe(`${BLOCK_HEIGHT + BLOCK_BLEED}px`);
  });

  it('gives removed text a short marker across the gap it left', () => {
    render(PREVIOUS_NOTE);

    // The gap is between the second and third blocks, 200px down the note.
    expect(marker('removed').style.top).toBe(`${2 * BLOCK_HEIGHT - GAP_MARKER_HEIGHT / 2}px`);
    expect(marker('removed').style.height).toBe(`${GAP_MARKER_HEIGHT}px`);
  });

  it('spans every block of a change', () => {
    render(PREVIOUS_NOTE, TWO_ADDED);

    expect(markers().map((el) => el.getAttribute('aria-label'))).toEqual([
      'Removed: 1 block',
      'Modified: 1 block',
      'Added: 2 blocks',
    ]);
    expect(marker('added').style.top).toBe(`${5 * BLOCK_HEIGHT}px`);
    expect(marker('added').style.height).toBe(`${2 * BLOCK_HEIGHT + BLOCK_BLEED}px`);
  });

  it('leaves the markers where they are as the note scrolls', () => {
    const { scrollEl } = render(PREVIOUS_NOTE);
    const placed = () => markers().map((el) => [el.style.top, el.style.height]);
    const before = placed();

    scrollEl.scrollTop = 250;
    scrollEl.dispatchEvent(new Event('scroll'));
    flushSync();

    expect(placed()).toEqual(before);
  });

  it('moves the markers when the note is laid out again', () => {
    const { contentEl, scrollEl } = render(PREVIOUS_NOTE);

    // Every block half as tall again: an image loaded, the text wrapped.
    layOut(contentEl, scrollEl, 300, 150);
    resize();
    flushSync();

    expect(marker('removed').style.top).toBe(`${2 * 150 - GAP_MARKER_HEIGHT / 2}px`);
    expect(marker('modified').style.top).toBe(`${3 * 150}px`);
    expect(marker('modified').style.height).toBe('150px');
    expect(marker('added').style.top).toBe(`${5 * 150}px`);
  });

  it('keeps its distance from the edge when the note grows a scrollbar', () => {
    const { scrollEl, props } = render(PREVIOUS_NOTE);
    props.showAll = true;
    flushSync();
    // The markers, and the rectangles that end at them.
    const scrollbar = () =>
      [bar()!, layer()!].map((el) => el.style.getPropertyValue('--note-diff-scrollbar')).join();
    expect(scrollbar()).toBe('0px,0px');

    // A scrollbar takes 6px from the scroll area's content.
    Object.defineProperties(scrollEl, {
      offsetWidth: { configurable: true, value: 700 },
      clientWidth: { configurable: true, value: 694 },
    });
    resize();
    flushSync();

    expect(scrollbar()).toBe('6px,6px');
  });

  it('paints one rectangle behind a change of several blocks, margins and all', () => {
    const { scrollEl, contentEl } = render(SECTION_PREVIOUS, SECTION_ADDED, 300, MARGIN);
    expect(markers().map((el) => el.getAttribute('aria-label'))).toEqual(['Added: 5 blocks']);
    // Nothing is painted until the change is pointed at.
    expect(blocks()).toEqual([]);

    hover(marker('added'));

    // One rectangle for the heading, the sentence and the three items, from
    // the heading's top to the last item's bottom: five blocks and the four
    // margins between them, and a little of the margin above and below.
    expect(blocks()).toHaveLength(1);
    const top = 5 * (BLOCK_HEIGHT + MARGIN) - BLOCK_BLEED;
    const height = 5 * BLOCK_HEIGHT + 4 * MARGIN + 2 * BLOCK_BLEED;
    expect(block('added').style.top).toBe(`${top}px`);
    expect(block('added').style.height).toBe(`${height}px`);
    expect(parts(block('added'))).toEqual([['added', `${height}px`]]);
    expect(block('added').classList.contains('is-active')).toBe(true);
    // It is beside the note's markup, not in it: in the scroll area, as the markers are.
    expect(layer()!.parentElement).toBe(scrollEl);
    expect(contentEl.querySelector('.note-diff-block')).toBeNull();
    expect(layer()!.getAttribute('aria-hidden')).toBe('true');

    leave(marker('added'));
    expect(blocks()).toEqual([]);
  });

  it('ends each rectangle at its marker, which is as tall as it', () => {
    const { props } = render(PREVIOUS_NOTE, TWO_ADDED, 300, MARGIN);
    props.showAll = true;
    flushSync();

    for (const kind of ['modified', 'added']) {
      expect(block(kind).style.top).toBe(marker(kind).style.top);
      expect(block(kind).style.height).toBe(marker(kind).style.height);
      expect(block(kind).dataset.hunkId).toBe(marker(kind).dataset.hunkId);
    }
  });

  it('keeps each block’s own colour within the rectangle of its change', () => {
    render(REWORDED_PREVIOUS, REWORDED_THEN_ADDED, 300, MARGIN);
    expect(markers().map((el) => el.dataset.kind)).toEqual(['modified']);

    hover(marker('modified'));

    // The reworded paragraph and the one added after it are one rectangle,
    // amber then green, meeting half-way across the margin between them.
    expect(blocks()).toHaveLength(1);
    expect(block('modified').style.height).toBe(`${2 * BLOCK_HEIGHT + MARGIN + 2 * BLOCK_BLEED}px`);
    expect(parts(block('modified'))).toEqual([
      ['modified', `${BLOCK_BLEED + BLOCK_HEIGHT + MARGIN / 2}px`],
      ['added', `${MARGIN / 2 + BLOCK_HEIGHT + BLOCK_BLEED}px`],
    ]);
  });

  it('paints every change beside the previous version, and the pointed one strongly', () => {
    const { props } = render(PREVIOUS_NOTE);
    props.showAll = true;
    flushSync();

    // The removed text has no block in the note: it has a line, not a rectangle.
    expect(blocks().map((el) => el.dataset.kind)).toEqual(['modified', 'added']);
    expect(blocks().map((el) => el.classList.contains('is-active'))).toEqual([false, false]);
    expect(gaps().map((el) => [el.dataset.kind, el.style.top])).toEqual([
      ['removed', `${2 * BLOCK_HEIGHT}px`],
    ]);

    hover(marker('added'));

    expect(blocks().map((el) => el.classList.contains('is-active'))).toEqual([false, true]);
  });

  it('draws the line for removed text with the rectangles, across its marker', () => {
    render(PREVIOUS_NOTE);
    expect(gaps()).toEqual([]);

    hover(marker('removed'));

    expect(gaps()).toHaveLength(1);
    expect(gaps()[0].parentElement!.parentElement).toBe(layer());
    expect(gaps()[0].classList.contains('is-active')).toBe(true);
    // The marker is centred on the line.
    expect(parseFloat(marker('removed').style.top) + GAP_MARKER_HEIGHT / 2).toBe(
      parseFloat(gaps()[0].style.top)
    );
  });

  it('moves the rectangles when the note is laid out again', () => {
    const { contentEl, scrollEl, props } = render(PREVIOUS_NOTE);
    props.showAll = true;
    flushSync();
    expect(block('modified').style.top).toBe(`${3 * BLOCK_HEIGHT}px`);

    layOut(contentEl, scrollEl, 300, 150);
    resize();
    flushSync();

    expect(block('modified').style.top).toBe(`${3 * 150}px`);
    expect(block('modified').style.height).toBe('150px');
    expect(gaps()[0].style.top).toBe(`${2 * 150}px`);
  });

  it('holds the rectangles and the markers still when the note scrolls sideways', () => {
    const { contentEl, scrollEl, props } = render(PREVIOUS_NOTE);
    props.showAll = true;
    flushSync();
    const overflow = () =>
      [layer()!, bar()!].map((el) => el.style.getPropertyValue('--note-diff-overflow')).join();
    expect(overflow()).toBe('0px,0px');

    // A table wider than the note's column: the text runs 450px past it,
    // and the scroll area scrolls until the table's end is at its edge,
    // which is its right padding less.
    scrollEl.style.paddingRight = '24px';
    Object.defineProperties(contentEl, {
      scrollWidth: { configurable: true, value: 1100 },
      clientWidth: { configurable: true, value: 650 },
    });
    resize();
    flushSync();

    expect(overflow()).toBe('426px,426px');

    // Text that runs into the padding and no further scrolls nothing.
    Object.defineProperties(contentEl, {
      scrollWidth: { configurable: true, value: 660 },
    });
    resize();
    flushSync();

    expect(overflow()).toBe('0px,0px');

    // jsdom lays nothing out, so the rules are read. Each is in a column
    // of its own that sticks sideways, inside an element as wide as what
    // the scroll area scrolls: the column slides along it and stays where
    // it was on screen.
    const rule = (source: string, selector: string) =>
      source.match(new RegExp(`\\n  ${selector.replace(/[.]/g, '\\.')} \\{[^}]*\\}`))?.[0] ?? '';
    expect(blocks().every((el) => el.parentElement!.parentElement === layer())).toBe(true);
    expect(blocks()[0].parentElement!.className).toContain('note-diff-blocks-column');
    expect(rule(blocksSource, '.note-diff-blocks')).toMatch(/var\(--note-diff-overflow, 0px\)/);
    expect(rule(blocksSource, '.note-diff-blocks-column')).toMatch(/position:\s*sticky/);
    expect(rule(blocksSource, '.note-diff-blocks-column')).toMatch(
      /width:\s*calc\(100% - var\(--note-diff-overflow, 0px\)/
    );
    expect(marker('added').parentElement!.parentElement).toBe(bar());
    expect(rule(markersSource, '.note-change-bar')).toMatch(/var\(--note-diff-overflow, 0px\)/);
    expect(rule(markersSource, '.note-change-markers')).toMatch(/position:\s*sticky/);
    expect(rule(markersSource, '.note-change-markers')).toMatch(
      /width:\s*calc\(100% - var\(--note-diff-overflow, 0px\)\)/
    );
  });

  it('lights a change while its marker is hovered, with the words it gained', () => {
    const { contentEl, noteDiff } = render(PREVIOUS_NOTE);

    hover(marker('modified'));

    expect(active(contentEl)).toEqual(['The cache is filled when the app starts.']);
    expect(paintedText(highlights, NOTE_DIFF_ADDED_HIGHLIGHT)).toEqual(['filled']);
    expect(marker('modified').classList.contains('is-pointed')).toBe(true);

    leave(marker('modified'));

    expect(active(contentEl)).toEqual([]);
    expect(highlights.has(NOTE_DIFF_ADDED_HIGHLIGHT)).toBe(false);
    expect(noteDiff.hoveredHunkId).toBeNull();
  });

  it('lights a change while its marker has keyboard focus', () => {
    const { contentEl } = render(PREVIOUS_NOTE);

    marker('added').focus();
    flushSync();
    expect(active(contentEl)).toEqual(['A hundred entries go at a time.']);

    marker('added').blur();
    flushSync();
    expect(active(contentEl)).toEqual([]);
  });

  it('does not light a change that scrolls under a pointer at rest', () => {
    const { contentEl, scrollEl } = render(PREVIOUS_NOTE);
    // The pointer comes to rest over the note's right edge.
    pointer('pointermove', scrollEl, 690, 120);

    // The note scrolls, and the browser reports the pointer, where it
    // already was, to the marker now under it.
    pointer('pointerenter', marker('added'), 690, 120);
    pointer('pointermove', marker('added'), 690, 120);
    expect(active(contentEl)).toEqual([]);

    // The pointer moves on the marker: that is a hover.
    pointer('pointermove', marker('added'), 690, 121);
    expect(active(contentEl)).toEqual(['A hundred entries go at a time.']);
  });

  it('does not light a change whose marker is under a pointer it has not seen move', () => {
    const { contentEl } = render(PREVIOUS_NOTE);

    // The first the bar sees of the pointer is a move reported to a marker.
    pointer('pointerenter', marker('added'), 690, 120);
    pointer('pointermove', marker('added'), 690, 120);
    expect(active(contentEl)).toEqual([]);

    pointer('pointermove', marker('added'), 690, 121);
    expect(active(contentEl)).toEqual(['A hundred entries go at a time.']);
  });

  it('lets go of a hovered change when its marker is no longer drawn', () => {
    const { contentEl, noteDiff } = render(PREVIOUS_NOTE);
    hover(marker('modified'));
    expect(active(contentEl)).toEqual(['The cache is filled when the app starts.']);

    // The note's blocks are not measured any more, so no marker is placed.
    // One that is removed is not left: no `pointerleave` comes for it.
    noteDiff.setBoxes('current', []);
    flushSync();
    expect(markers()).toEqual([]);

    expect(noteDiff.hoveredHunkId).toBeNull();
    expect(active(contentEl)).toEqual([]);
  });

  it('lets go of a change lit by focus when the markers are taken down', () => {
    const { noteDiff } = render(PREVIOUS_NOTE);
    marker('added').focus();
    flushSync();
    expect(noteDiff.hoveredHunkId).toBe(noteDiff.hunks[2].id);

    unmount(component!);
    component = null;

    expect(noteDiff.hoveredHunkId).toBeNull();
  });

  it('does not light a change for a touch, which has no hover to end', () => {
    const { contentEl } = render(PREVIOUS_NOTE);

    pointerArrives(marker('modified'), 'touch');
    flushSync();

    expect(active(contentEl)).toEqual([]);
  });

  it('repaints the words when the text under them is rewrapped', async () => {
    const { contentEl } = render(PREVIOUS_NOTE);
    hover(marker('modified'));
    const before = highlights.get(NOTE_DIFF_ADDED_HIGHLIGHT);

    // A quote's span goes round part of the paragraph, as a reply draft's does.
    const text = paragraph(contentEl, 'The cache is filled when the app starts.')
      .firstChild as Text;
    const rest = text.splitText('The cache is fil'.length);
    const span = document.createElement('span');
    rest.replaceWith(span);
    span.append(rest);
    await Promise.resolve();

    expect(highlights.get(NOTE_DIFF_ADDED_HIGHLIGHT)).not.toBe(before);
    expect(paintedText(highlights, NOTE_DIFF_ADDED_HIGHLIGHT)).toEqual(['filled']);
  });

  it('reports a pressed marker, and scrolls the note to the change once it is selected', () => {
    const { noteDiff, scrollEl, onSelect } = render(PREVIOUS_NOTE);
    const hunk = noteDiff.hunks[2];

    marker('added').click();
    flushSync();
    expect(onSelect).toHaveBeenCalledExactlyOnceWith(hunk.id);
    // Selecting is the viewer's to do.
    expect(noteDiff.activeHunkId).toBeNull();

    noteDiff.select(hunk.id, false);
    flushSync();

    // The change's top, 500px down, on the line a third of the way down 300px:
    // as far as 600px of note scrolls in it.
    expect(scrollEl.scrollTop).toBe(300);
    expect(marker('added').getAttribute('aria-pressed')).toBe('true');
    expect(marker('added').classList.contains('is-pointed')).toBe(true);
  });

  it('scrolls the note to the gap a removal left while the previous version is not shown', () => {
    const { noteDiff, scrollEl } = render(PREVIOUS_NOTE);

    noteDiff.select(noteDiff.hunks[0].id, false);
    flushSync();

    // The gap is 200px down the note, above "See the design doc".
    expect(scrollEl.scrollTop).toBe(100);
  });

  it('leaves a removal to the previous version’s pane once that is measured', () => {
    const { noteDiff, contentEl, scrollEl } = render(PREVIOUS_NOTE);
    // The previous version is shown and laid out as the note is: its pane
    // puts the removed paragraph on its line, and the note follows it.
    noteDiff.setBoxes(
      'previous',
      noteDiff.previousUnits!.map((_, index) => ({ top: index * 100, height: 100 }))
    );
    scrollEl.scrollTop = 40;

    noteDiff.select(noteDiff.hunks[0].id, true);
    flushSync();
    expect(scrollEl.scrollTop).toBe(40);

    // Nor on a new layout.
    layOut(contentEl, scrollEl, 150);
    resize();
    expect(scrollEl.scrollTop).toBe(40);

    // A change the note has blocks for is still the note's to place.
    noteDiff.select(noteDiff.hunks[1].id, true);
    flushSync();
    expect(scrollEl.scrollTop).toBe(250);
  });

  it('keeps the selected change on its line through a new layout, until the reader scrolls', () => {
    const { noteDiff, contentEl, scrollEl } = render(PREVIOUS_NOTE);
    noteDiff.select(noteDiff.hunks[1].id, false);
    flushSync();
    expect(scrollEl.scrollTop).toBe(200);

    // The pane got shorter: the line is higher up, and the note scrolls further.
    layOut(contentEl, scrollEl, 150);
    resize();
    expect(scrollEl.scrollTop).toBe(250);

    scrollEl.dispatchEvent(new Event('wheel'));
    scrollEl.scrollTop = 0;
    layOut(contentEl, scrollEl, 300);
    resize();
    expect(scrollEl.scrollTop).toBe(0);
  });

  it('marks the new blocks after the note renders again, without comparing again', () => {
    const { props, noteDiff, contentEl, scrollEl } = render(PREVIOUS_NOTE);
    const diff = noteDiff.diff;
    hover(marker('modified'));

    contentEl.innerHTML = renderNoteMarkdown(CURRENT_NOTE);
    layOut(contentEl, scrollEl, 300);
    props.renderKey = 1;
    flushSync();

    expect(noteDiff.diff).toBe(diff);
    expect(marked(contentEl)).toHaveLength(2);
    // Still hovered: the pointer has not moved.
    expect(active(contentEl)).toEqual(['The cache is filled when the app starts.']);
    expect(markers()).toHaveLength(3);
  });

  it('drops the markers and the marks when the previous version goes', () => {
    const { props, contentEl } = render(PREVIOUS_NOTE);
    hover(marker('modified'));

    props.previousHtml = null;
    flushSync();

    expect(bar()).toBeNull();
    expect(marked(contentEl)).toEqual([]);
    expect(active(contentEl)).toEqual([]);
    expect(highlights.has(NOTE_DIFF_ADDED_HIGHLIGHT)).toBe(false);
  });

  it('takes its marks off the note when it is unmounted', () => {
    const { contentEl } = render(PREVIOUS_NOTE);
    hover(marker('modified'));

    unmount(component!);
    component = null;

    expect(marked(contentEl)).toEqual([]);
    expect(active(contentEl)).toEqual([]);
    expect(highlights.has(NOTE_DIFF_ADDED_HIGHLIGHT)).toBe(false);
  });
});
