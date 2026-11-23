import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import NoteChangeBar from './NoteChangeBar.svelte';
import {
  NOTE_DIFF_ACTIVE_ATTRIBUTE,
  NOTE_DIFF_ADDED_HIGHLIGHT,
  NOTE_DIFF_ATTRIBUTE,
} from './noteDiffHighlights';
import { NoteDiffState } from './noteDiffState.svelte';
import {
  BLOCK_HEIGHT,
  CURRENT_NOTE,
  FakeHighlight,
  fakeResizeObserver,
  HUNK_KINDS,
  layOut,
  paintedText,
  PREVIOUS_NOTE,
} from './testStubs/noteDiffFixtures';
import { renderNoteMarkdown } from './noteMarkdown';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

/**
 * The change bar beside a note: one marker per hunk of the diff against the
 * previous version, placed and coloured, lighting its change on hover. The
 * note here is plain markup in a scroller, laid out by stubbed offsets.
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

function render(previous: string | null, current = CURRENT_NOTE, viewport = 300) {
  const scrollEl = document.createElement('div');
  const contentEl = document.createElement('div');
  contentEl.innerHTML = renderNoteMarkdown(current);
  scrollEl.append(contentEl);
  const target = document.createElement('div');
  document.body.append(scrollEl, target);
  layOut(contentEl, scrollEl, viewport);

  const noteDiff = new NoteDiffState();
  const onSelect = vi.fn<(hunkId: string) => void>();
  const props = reactiveProps({
    noteDiff,
    contentEl,
    scrollEl,
    renderKey: 0,
    previousHtml: previous === null ? null : renderNoteMarkdown(previous),
    onSelect,
  });
  component = mount(NoteChangeBar, { target, props });
  flushSync();
  return { props, noteDiff, contentEl, scrollEl, onSelect };
}

const bar = () => document.querySelector<HTMLElement>('.note-change-bar');
const markers = () => [...document.querySelectorAll<HTMLButtonElement>('.note-change-marker')];
const marker = (kind: string) => markers().find((el) => el.dataset.kind === kind)!;
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
  el.dispatchEvent(new MouseEvent('pointerenter'));
  flushSync();
}

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

  it('places each marker where its change is, scaled to the scrolled height', () => {
    // Six blocks of 100px in a 300px viewport: the bar is half the note's scale.
    render(PREVIOUS_NOTE);
    const scale = 300 / (6 * BLOCK_HEIGHT);

    // Removed: the gap above the third block, as short as a marker gets.
    expect(marker('removed').style.top).toBe(`${2 * BLOCK_HEIGHT * scale}px`);
    expect(marker('removed').style.height).toBe('4px');
    expect(marker('modified').style.top).toBe(`${3 * BLOCK_HEIGHT * scale}px`);
    expect(marker('modified').style.height).toBe(`${BLOCK_HEIGHT * scale}px`);
    expect(marker('added').style.top).toBe(`${5 * BLOCK_HEIGHT * scale}px`);
  });

  it('puts the markers beside their blocks when the note fits without scrolling', () => {
    render(PREVIOUS_NOTE, CURRENT_NOTE, 800);

    expect(marker('removed').style.top).toBe(`${2 * BLOCK_HEIGHT}px`);
    expect(marker('modified').style.top).toBe(`${3 * BLOCK_HEIGHT}px`);
    expect(marker('modified').style.height).toBe(`${BLOCK_HEIGHT}px`);
    expect(marker('added').style.top).toBe(`${5 * BLOCK_HEIGHT}px`);
  });

  it('moves the markers when the note is laid out again', () => {
    const { contentEl, scrollEl } = render(PREVIOUS_NOTE);

    layOut(contentEl, scrollEl, 600);
    resize();
    flushSync();

    expect(marker('modified').style.top).toBe(`${3 * BLOCK_HEIGHT}px`);
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

  it('does not light a change for a touch, which has no hover to end', () => {
    const { contentEl } = render(PREVIOUS_NOTE);

    const touch = new MouseEvent('pointerenter');
    Object.defineProperty(touch, 'pointerType', { value: 'touch' });
    marker('modified').dispatchEvent(touch);
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
