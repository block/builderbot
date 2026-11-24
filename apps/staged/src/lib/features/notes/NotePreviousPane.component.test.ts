import { createRawSnippet, flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import NotePreviousPane from './NotePreviousPane.svelte';
import paneSource from './NotePreviousPane.svelte?raw';
import {
  NOTE_DIFF_ACTIVE_ATTRIBUTE,
  NOTE_DIFF_ATTRIBUTE,
  NOTE_DIFF_REMOVED_HIGHLIGHT,
} from './noteDiffHighlights';
import {
  CONNECTOR_PREVIOUS_REACH,
  CONNECTOR_WIDTH,
  layoutNoteDiffConnectors,
} from './noteDiffConnectors';
import { BLOCK_BLEED } from './noteDiffScroll';
import { NoteDiffState } from './noteDiffState.svelte';
import { extractNoteDiffUnits, measureNoteDiffUnits } from './noteDiffUnits';
import { renderNoteMarkdown } from './noteMarkdown';
import {
  BLOCK_HEIGHT,
  CURRENT_NOTE,
  FakeHighlight,
  fakeResizeObserver,
  layOut,
  paintedText,
  pointerArrives,
  PREVIOUS_NOTE,
} from './testStubs/noteDiffFixtures';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

/**
 * The previous version beside the note: marked from the viewer's diff and
 * scrolling with the note. The note here is plain markup in a scroller of
 * its own, standing in for the viewer's pane; both are laid out by stubbed
 * offsets, six blocks of 100px in a 300px viewport.
 */

const VIEWPORT = 300;

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
  vi.spyOn(console, 'info').mockImplementation(() => {});
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

interface RenderOptions {
  /** What the pane shows, when it is not the version the diff was computed from. */
  shown?: string;
  /** Where the note is scrolled to when the pane opens. */
  currentScrollTop?: number;
  bottomInset?: number;
  /** The room between one block and the next, in both versions. */
  margin?: number;
}

function render({
  shown = PREVIOUS_NOTE,
  currentScrollTop = 0,
  bottomInset = 0,
  margin = 0,
}: RenderOptions = {}) {
  // The note, as the viewer and its change bar leave it: read, compared and measured.
  const currentScrollEl = document.createElement('div');
  const currentEl = document.createElement('div');
  currentEl.innerHTML = renderNoteMarkdown(CURRENT_NOTE);
  currentScrollEl.append(currentEl);
  const target = document.createElement('div');
  document.body.append(target, currentScrollEl);
  layOut(currentEl, currentScrollEl, VIEWPORT, BLOCK_HEIGHT, margin);
  currentScrollEl.scrollTop = currentScrollTop;

  const noteDiff = new NoteDiffState();
  const currentUnits = extractNoteDiffUnits(currentEl);
  noteDiff.compare(renderNoteMarkdown(PREVIOUS_NOTE), currentUnits);
  noteDiff.setBoxes('current', measureNoteDiffUnits(currentUnits, currentScrollEl));

  const html = renderNoteMarkdown(shown);
  const onClose = vi.fn();
  const onSelect = vi.fn<(hunkId: string) => void>();
  const onContentClick = vi.fn<(event: MouseEvent) => void>();
  const props = reactiveProps({
    noteDiff,
    contentEl: undefined as HTMLElement | undefined,
    renderKey: html,
    savedAt: Date.now() - 5 * 60_000,
    currentScrollEl,
    bottomInset,
    onClose,
    onSelect,
    onContentClick,
    content: createRawSnippet(() => ({
      render: () => `<div class="markdown-content">${html}</div>`,
    })),
  });
  component = mount(NotePreviousPane, { target, props });
  flushSync();

  // The viewer binds the element its snippet rendered and hands it back.
  const scrollEl = document.querySelector<HTMLElement>('.previous-scroll')!;
  const contentEl = scrollEl.querySelector<HTMLElement>('.markdown-content')!;
  layOut(contentEl, scrollEl, VIEWPORT, BLOCK_HEIGHT, margin);
  props.contentEl = contentEl;
  flushSync();
  return {
    props,
    noteDiff,
    contentEl,
    scrollEl,
    currentScrollEl,
    onClose,
    onSelect,
    onContentClick,
  };
}

const marked = (el: Element) =>
  [...el.querySelectorAll(`[${NOTE_DIFF_ATTRIBUTE}]`)].map((block) => [
    block.textContent,
    block.getAttribute(NOTE_DIFF_ATTRIBUTE),
  ]);
const active = (el: Element) =>
  [...el.querySelectorAll(`[${NOTE_DIFF_ACTIVE_ATTRIBUTE}]`)].map((block) => block.textContent);
const gaps = () => [...document.querySelectorAll<HTMLElement>('.note-diff-gap')];
/** The rectangles painted behind this version's changes. */
const blocks = () => [...document.querySelectorAll<HTMLElement>('.note-diff-block')];
const block = (kind: string) => blocks().find((el) => el.dataset.kind === kind)!;
const connectors = () => [...document.querySelectorAll<SVGGElement>('.note-diff-connector')];
const connector = (kind: string) => connectors().find((el) => el.dataset.kind === kind)!;
const band = (el: Element) => el.querySelector<SVGPathElement>('.connector-fill')!;
/** Where a band meets each pane, read back from its path: "M 0 y C … width y L width y C … 0 y Z". */
function bandEdges(el: Element) {
  const d = band(el).getAttribute('d')!;
  const n = d.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
  // A band that tapers on the right has no line down that edge.
  const afterBottom = d.includes('L') ? n[9] : n[7];
  return { beforeTop: n[1], beforeBottom: n[n.length - 1], afterTop: n[7], afterBottom };
}

/**
 * The reader scrolls a pane: the offset, then the event the browser sends
 * for it. What is drawn from the panes' offsets is brought up to date, as it
 * is before the browser next paints.
 */
function scrollTo(el: HTMLElement, top: number) {
  el.scrollTop = top;
  el.dispatchEvent(new Event('scroll'));
  flushSync();
}

describe('NotePreviousPane', () => {
  it('names the version and when it was saved, and closes from its header', () => {
    const { onClose } = render();
    const pane = document.querySelector('.previous-pane')!;

    expect(pane.getAttribute('aria-label')).toBe('Previous version');
    expect(pane.querySelector('.note-diff-pane-header')!.textContent).toMatch(
      /Previous version\s*· saved 5m ago/
    );

    pane.querySelector<HTMLButtonElement>('button[aria-label="Close previous version"]')!.click();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('marks the blocks the revision removed or changed, and where it added text', () => {
    const { contentEl, scrollEl } = render();

    expect(marked(contentEl)).toEqual([
      ['Logs are kept for a week.', 'removed'],
      ['The cache is warmed when the app starts.', 'modified'],
    ]);
    // Every change is tinted here, not only the one pointed at.
    expect(scrollEl.classList.contains('note-diff-all')).toBe(true);
    // The added paragraph has no block here: a line under the last one.
    expect(gaps().map((gap) => [gap.dataset.kind, gap.style.top])).toEqual([
      ['added', `${6 * BLOCK_HEIGHT + 4}px`],
    ]);
    expect(gaps()[0].classList.contains('is-active')).toBe(false);
  });

  it('paints one rectangle behind each change, out to the gutter', () => {
    const { noteDiff, scrollEl, contentEl } = render({ margin: 20 });

    // One per change with blocks here, from just above its block to just
    // below, whichever blocks it holds.
    expect(blocks().map((el) => [el.dataset.kind, el.style.top, el.style.height])).toEqual([
      // The removed paragraph is a block here, the third.
      [
        'removed',
        `${2 * (BLOCK_HEIGHT + 20) - BLOCK_BLEED}px`,
        `${BLOCK_HEIGHT + 2 * BLOCK_BLEED}px`,
      ],
      [
        'modified',
        `${4 * (BLOCK_HEIGHT + 20) - BLOCK_BLEED}px`,
        `${BLOCK_HEIGHT + 2 * BLOCK_BLEED}px`,
      ],
    ]);
    expect(blocks().map((el) => el.dataset.hunkId)).toEqual([
      noteDiff.hunks[0].id,
      noteDiff.hunks[1].id,
    ]);
    // Behind the version, not in it: the rendered markdown holds what the
    // note's author wrote and nothing else.
    const layer = scrollEl.querySelector<HTMLElement>(':scope > .note-diff-blocks')!;
    expect(blocks().every((el) => el.parentElement!.parentElement === layer)).toBe(true);
    expect(contentEl.querySelector('.note-diff-block, .note-diff-gap')).toBeNull();
    expect(layer.getAttribute('aria-hidden')).toBe('true');
    expect(layer.dataset.side).toBe('previous');
    // The layer stops where the gutter starts, and the bands go on from it.
    expect(scrollEl.style.getPropertyValue('--note-diff-blocks-right')).toBe(
      `${CONNECTOR_PREVIOUS_REACH}px`
    );
  });

  it('has no markers: a change is pointed at and selected from its band or the note', () => {
    const { noteDiff, scrollEl } = render();

    expect(scrollEl.querySelector('.note-change-bar, .note-change-marker')).toBeNull();
    expect(document.querySelectorAll('.previous-pane button')).toHaveLength(1);
    // Every change has something drawn for it here all the same.
    expect(blocks().length + gaps().length).toBe(noteDiff.hunks.length);
  });

  it('ends each band on the rectangle it starts from', () => {
    const { noteDiff, scrollEl } = render({ margin: 20 });

    // The panes show 300px: the removed paragraph, tapering to its gap.
    const edge = (el: HTMLElement) => parseFloat(el.style.top) - scrollEl.scrollTop;
    const height = (el: HTMLElement) => parseFloat(el.style.height);
    let edges = bandEdges(connector('removed'));
    expect(edges.beforeTop).toBe(edge(block('removed')));
    expect(edges.beforeBottom).toBe(edge(block('removed')) + height(block('removed')));

    // Scrolled, the band's end moves with the rectangle.
    scrollTo(scrollEl, 190);
    edges = bandEdges(connector('removed'));
    expect(edges.beforeTop).toBe(edge(block('removed')));
    expect(edges.beforeBottom).toBe(edge(block('removed')) + height(block('removed')));
    edges = bandEdges(connector('modified'));
    expect(edges.beforeTop).toBe(edge(block('modified')));
    expect(edges.beforeBottom).toBe(edge(block('modified')) + height(block('modified')));

    // The added paragraph has no block here: its band ends on the gap line.
    scrollTo(scrollEl, 420);
    edges = bandEdges(connector('added'));
    const gap = parseFloat(gaps()[0].style.top) - scrollEl.scrollTop;
    expect(gaps()[0].dataset.hunkId).toBe(noteDiff.hunks[2].id);
    expect([edges.beforeTop, edges.beforeBottom]).toEqual([gap, gap]);
  });

  it('lights the change being pointed at, with the words it lost', () => {
    const { noteDiff, contentEl } = render();

    noteDiff.select(noteDiff.hunks[1].id, true);
    flushSync();
    expect(active(contentEl)).toEqual(['The cache is warmed when the app starts.']);
    expect(paintedText(highlights, NOTE_DIFF_REMOVED_HIGHLIGHT)).toEqual(['warmed']);

    // The hovered change shows over the selected one.
    noteDiff.hover(noteDiff.hunks[2].id);
    flushSync();
    expect(active(contentEl)).toEqual([]);
    expect(gaps()[0].classList.contains('is-active')).toBe(true);

    noteDiff.hover(null);
    noteDiff.close();
    flushSync();
    expect(active(contentEl)).toEqual([]);
    expect(highlights.has(NOTE_DIFF_REMOVED_HIGHLIGHT)).toBe(false);
  });

  it('leaves a version unmarked when its blocks are not the ones the diff compared', () => {
    const { noteDiff, contentEl } = render({ shown: `${PREVIOUS_NOTE}\n\nSomething else.` });

    noteDiff.select(noteDiff.hunks[1].id, true);
    flushSync();

    expect(marked(contentEl)).toEqual([]);
    expect(active(contentEl)).toEqual([]);
    expect(noteDiff.boxes.previous).toEqual([]);
  });

  it('opens where the note is', () => {
    // The note's reference line, 250px down it, is halfway through "See the
    // design doc": 350px down this version, which has a paragraph more above.
    const { scrollEl } = render({ currentScrollTop: 150 });

    expect(scrollEl.scrollTop).toBe(250);
  });

  it('follows the note as it scrolls', () => {
    const { scrollEl, currentScrollEl } = render();
    expect(scrollEl.scrollTop).toBe(0);

    scrollTo(currentScrollEl, 150);
    expect(scrollEl.scrollTop).toBe(250);

    scrollTo(currentScrollEl, 50);
    expect(scrollEl.scrollTop).toBe(50);
  });

  it('leads the note when it is the one scrolled, the note holding still past removed text', () => {
    const { scrollEl, currentScrollEl } = render();

    // Its line is at the top of the removed paragraph, then halfway down it:
    // the note has nothing there, and waits at the block that follows.
    scrollTo(scrollEl, 100);
    expect(currentScrollEl.scrollTop).toBe(100);
    scrollTo(scrollEl, 150);
    expect(currentScrollEl.scrollTop).toBe(100);

    scrollTo(scrollEl, 250);
    expect(currentScrollEl.scrollTop).toBe(150);
  });

  it('starts a selected removal on its line, and puts the note at the gap', () => {
    const { noteDiff, scrollEl, currentScrollEl } = render();

    // "Logs are kept for a week" is 200px down this version and the line is
    // 100px down the pane. The note has no such block: placed from the note,
    // this pane would show the block after it and the paragraph would end on
    // the line instead of starting there.
    noteDiff.select(noteDiff.hunks[0].id, true);
    flushSync();

    expect(scrollEl.scrollTop).toBe(100);
    // The gap, 200px down the note, on the note's line.
    expect(currentScrollEl.scrollTop).toBe(100);

    // The browser reports both moves: neither is answered.
    scrollEl.dispatchEvent(new Event('scroll'));
    currentScrollEl.dispatchEvent(new Event('scroll'));
    expect(scrollEl.scrollTop).toBe(100);
    expect(currentScrollEl.scrollTop).toBe(100);
  });

  it('is not pulled off a selected removal by the note’s scroll to the gap', () => {
    // The change bar has put the gap on the note's line already, as it does
    // before this pane is measured, and the event for that is still to come.
    const { noteDiff, scrollEl, currentScrollEl } = render({ currentScrollTop: 100 });
    expect(scrollEl.scrollTop).toBe(200);

    noteDiff.select(noteDiff.hunks[0].id, true);
    flushSync();
    expect(scrollEl.scrollTop).toBe(100);
    expect(currentScrollEl.scrollTop).toBe(100);

    // The note is where this pane wants it, so the event tells it nothing:
    // answered, it would put the block after the removed text on the line.
    currentScrollEl.dispatchEvent(new Event('scroll'));
    expect(scrollEl.scrollTop).toBe(100);
  });

  it('keeps a selected removal on its line through a new layout, until the reader scrolls', () => {
    const { noteDiff, contentEl, scrollEl, currentScrollEl } = render();
    noteDiff.select(noteDiff.hunks[0].id, true);
    flushSync();
    expect(scrollEl.scrollTop).toBe(100);

    // Half the viewport: the line is 50px down instead of 100.
    layOut(contentEl, scrollEl, VIEWPORT / 2);
    Object.defineProperty(currentScrollEl, 'clientHeight', { configurable: true, value: 150 });
    resize();
    flushSync();
    expect(scrollEl.scrollTop).toBe(150);
    expect(currentScrollEl.scrollTop).toBe(150);

    // The reader scrolls this pane themselves: it stays where they put it.
    scrollEl.dispatchEvent(new Event('wheel'));
    scrollTo(scrollEl, 0);
    expect(currentScrollEl.scrollTop).toBe(0);
    layOut(contentEl, scrollEl, VIEWPORT);
    Object.defineProperty(currentScrollEl, 'clientHeight', { configurable: true, value: 300 });
    resize();
    flushSync();
    expect(scrollEl.scrollTop).toBe(0);
    expect(currentScrollEl.scrollTop).toBe(0);
  });

  it('leaves a selected change the note has blocks for to the note', () => {
    const { noteDiff, scrollEl, currentScrollEl } = render();

    // The reworded paragraph is placed by the note's change bar, which is
    // not here: nothing moves until the note does.
    noteDiff.select(noteDiff.hunks[1].id, true);
    flushSync();
    expect(scrollEl.scrollTop).toBe(0);

    scrollTo(currentScrollEl, 200);
    expect(scrollEl.scrollTop).toBe(300);
  });

  it('does not answer the scroll it caused', () => {
    const { scrollEl, currentScrollEl } = render();

    // The note at its end asks for more than this version can scroll.
    scrollTo(currentScrollEl, 300);
    expect(scrollEl.scrollTop).toBe(300);

    // The browser reports the pane's move. Mapped back, it would pull the note up.
    scrollEl.dispatchEvent(new Event('scroll'));
    expect(currentScrollEl.scrollTop).toBe(300);

    // A scroll of the pane that is the reader's own is answered.
    scrollTo(scrollEl, 150);
    expect(currentScrollEl.scrollTop).toBe(100);
  });

  it('is put back in step when a pane is laid out again', () => {
    const { contentEl, scrollEl, currentScrollEl } = render({ currentScrollTop: 150 });
    expect(scrollEl.scrollTop).toBe(250);

    // Half the viewport: the line is 50px down instead of 100.
    layOut(contentEl, scrollEl, VIEWPORT / 2);
    Object.defineProperty(currentScrollEl, 'clientHeight', { configurable: true, value: 150 });
    resize();
    flushSync();

    // The note's line, 200px down it, is the top of "See the design doc":
    // 300px down this version.
    expect(scrollEl.scrollTop).toBe(250);
  });

  it('joins each change to its place in the note with a band between the panes', () => {
    const { noteDiff, scrollEl } = render();

    // In the gutter, beside the scroll area and not in it: the bands follow
    // both panes, and scroll with neither.
    const svg = document.querySelector('.note-diff-connectors')!;
    expect(svg.parentElement).toBe(scrollEl.parentElement);
    expect(scrollEl.contains(svg)).toBe(false);

    // The panes show 300px at the top: the removed paragraph, the third
    // block here, tapering to the gap it left above the note's third.
    expect(connectors().map((el) => el.dataset.hunkId)).toEqual([noteDiff.hunks[0].id]);
    expect(bandEdges(connector('removed'))).toEqual({
      beforeTop: 2 * BLOCK_HEIGHT,
      beforeBottom: 3 * BLOCK_HEIGHT,
      afterTop: 2 * BLOCK_HEIGHT,
      afterBottom: 2 * BLOCK_HEIGHT,
    });
  });

  it('moves the bands as the note scrolls, each end with its own pane', () => {
    const { noteDiff, scrollEl, currentScrollEl } = render();

    scrollTo(currentScrollEl, 150);
    expect(scrollEl.scrollTop).toBe(250);

    // The removal: its paragraph is 250px further up this pane, its gap
    // 150px further up the note.
    expect(bandEdges(connector('removed'))).toEqual({
      beforeTop: -50,
      beforeBottom: 50,
      afterTop: 50,
      afterBottom: 50,
    });
    // The reworded paragraph, the fifth block here and the note's fourth,
    // has come into view level in both.
    expect(bandEdges(connector('modified'))).toEqual({
      beforeTop: 150,
      beforeBottom: 250,
      afterTop: 150,
      afterBottom: 250,
    });
    // What is drawn is the layout for where both panes now are.
    expect(connectors().map((el) => band(el).getAttribute('d'))).toEqual(
      layoutNoteDiffConnectors(noteDiff.hunks, noteDiff.boxes, {
        scrollTops: { previous: 250, current: 150 },
        width: CONNECTOR_WIDTH,
        height: VIEWPORT,
      }).map(({ fill }) => fill)
    );
  });

  it('moves the bands as this pane scrolls, the note’s end holding still with the note', () => {
    const { scrollEl, currentScrollEl } = render();

    scrollTo(scrollEl, 100);
    expect(currentScrollEl.scrollTop).toBe(100);
    expect(bandEdges(connector('removed'))).toMatchObject({ beforeTop: 100, afterTop: 100 });

    // Through the removed paragraph the note waits, and only this end moves:
    // the two sides of the change stand at different heights.
    scrollTo(scrollEl, 150);
    expect(currentScrollEl.scrollTop).toBe(100);
    expect(bandEdges(connector('removed'))).toEqual({
      beforeTop: 50,
      beforeBottom: 150,
      afterTop: 100,
      afterBottom: 100,
    });
  });

  it('draws the bands from where the note is when it opens', () => {
    render({ currentScrollTop: 150 });

    expect(bandEdges(connector('modified'))).toMatchObject({ beforeTop: 150, afterTop: 150 });
  });

  it('leads a change scrolled out of one pane off the edge, and drops it once out of both', () => {
    const { scrollEl } = render();

    // The removed paragraph's top has left this pane; its bottom, and the
    // gap in the note, are still in view.
    scrollTo(scrollEl, 280);
    const edges = bandEdges(connector('removed'));
    expect(edges.beforeTop).toBeLessThan(0);
    expect(edges.beforeBottom).toBeGreaterThan(0);

    scrollTo(scrollEl, 400);
    expect(connectors().map((el) => el.dataset.kind)).not.toContain('removed');
  });

  it('moves the bands when a pane is laid out again', () => {
    const { contentEl, scrollEl } = render();
    expect(bandEdges(connector('removed')).beforeTop).toBe(2 * BLOCK_HEIGHT);

    // This version's blocks are half as tall: an image above came in smaller.
    layOut(contentEl, scrollEl, VIEWPORT, BLOCK_HEIGHT / 2);
    resize();
    flushSync();

    expect(bandEdges(connector('removed'))).toMatchObject({
      beforeTop: BLOCK_HEIGHT,
      beforeBottom: 1.5 * BLOCK_HEIGHT,
      afterTop: 2 * BLOCK_HEIGHT,
    });
  });

  it('holds the version in a box that scrolls sideways, with the rectangles and gap lines outside it', () => {
    const { scrollEl, contentEl } = render();

    const box = scrollEl.querySelector<HTMLElement>(':scope > .previous-content')!;
    expect(box.contains(contentEl)).toBe(true);
    // What marks the changes is placed in the scroll area, across its
    // width: it is not in the box, and stays put when the version is
    // scrolled sideways.
    expect(box.querySelector('.note-diff-blocks, .note-diff-block, .note-diff-gap')).toBeNull();
    expect(scrollEl.querySelectorAll(':scope > .note-diff-blocks')).toHaveLength(1);
    expect(
      scrollEl.querySelectorAll(':scope > .note-diff-blocks > * > .note-diff-gap')
    ).toHaveLength(1);
    expect(
      scrollEl.querySelectorAll(':scope > .note-diff-blocks > * > .note-diff-block')
    ).toHaveLength(2);

    // jsdom lays nothing out, so the rules are read. The scroll area keeps
    // its scrollbar on the left by being laid out right to left, which puts
    // what runs past its right edge out of reach: it does not scroll
    // sideways, and the box inside it, laid out left to right, does.
    const css = /<style[^>]*>([\s\S]*)<\/style>/
      .exec(paneSource)![1]
      .replace(/\/\*[\s\S]*?\*\//g, '');
    const rule = (selector: string) =>
      new RegExp(`(?:^|})\\s*${selector.replace(/[.*>()]/g, '\\$&')}\\s*\\{([^}]*)\\}`).exec(
        css
      )?.[1] ?? '';
    expect(rule('.previous-scroll')).toMatch(/direction:\s*rtl/);
    expect(rule('.previous-scroll')).toMatch(/overflow-y:\s*auto/);
    expect(rule('.previous-scroll')).toMatch(/overflow-x:\s*hidden/);
    expect(rule('.previous-scroll > :global(*)')).toMatch(/direction:\s*ltr/);
    expect(rule('.previous-content')).toMatch(/overflow-x:\s*auto/);
  });

  it('lets go of a change hovered at its band when the pane closes', () => {
    const { noteDiff } = render();

    pointerArrives(band(connector('removed')));
    flushSync();
    expect(noteDiff.hoveredHunkId).toBe(noteDiff.hunks[0].id);
    unmount(component!);
    component = null;
    expect(noteDiff.hoveredHunkId).toBeNull();
  });

  it('lights a change in both panes from its band, and reports a press on it', () => {
    const { noteDiff, contentEl, onSelect } = render();

    pointerArrives(band(connector('removed')));
    flushSync();
    expect(noteDiff.pointedHunkId).toBe(noteDiff.hunks[0].id);
    expect(active(contentEl)).toEqual(['Logs are kept for a week.']);
    expect(connector('removed').classList.contains('is-pointed')).toBe(true);
    expect(block('removed').classList.contains('is-active')).toBe(true);

    band(connector('removed')).dispatchEvent(new MouseEvent('pointerleave'));
    flushSync();
    expect(active(contentEl)).toEqual([]);
    expect(connector('removed').classList.contains('is-pointed')).toBe(false);
    expect(block('removed').classList.contains('is-active')).toBe(false);

    band(connector('removed')).dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(onSelect).toHaveBeenCalledExactlyOnceWith(noteDiff.hunks[0].id);
  });

  it('adds no Tab stop for the bands', () => {
    render();

    const svg = document.querySelector('.note-diff-connectors')!;
    expect(svg.getAttribute('aria-hidden')).toBe('true');
    expect(svg.querySelectorAll('[tabindex], button, a')).toHaveLength(0);
  });

  it('scrolls the note when the wheel is turned over a band, and follows it', () => {
    const { scrollEl, currentScrollEl } = render();

    band(connector('removed')).dispatchEvent(
      new WheelEvent('wheel', { deltaY: 150, bubbles: true })
    );
    expect(currentScrollEl.scrollTop).toBe(150);

    // The browser reports the note's move, and this pane answers it.
    currentScrollEl.dispatchEvent(new Event('scroll'));
    flushSync();
    expect(scrollEl.scrollTop).toBe(250);
  });

  it('passes presses in the version to the viewer', () => {
    const { contentEl, onContentClick } = render();

    contentEl.querySelector('p')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));

    expect(onContentClick).toHaveBeenCalledOnce();
  });

  it('keeps clear of what stands under the note’s scroll area', () => {
    render({ bottomInset: 64 });

    expect(document.querySelector<HTMLElement>('.previous-bottom')!.style.height).toBe('64px');
  });

  it('has nothing under it when the note has not', () => {
    render();

    expect(document.querySelector('.previous-bottom')).toBeNull();
  });

  it('gives up its measurements and its marks when it closes', () => {
    const { noteDiff, contentEl } = render();
    noteDiff.select(noteDiff.hunks[1].id, true);
    flushSync();
    expect(noteDiff.boxes.previous).toHaveLength(6);

    unmount(component!);
    component = null;

    expect(noteDiff.boxes.previous).toEqual([]);
    expect(marked(contentEl)).toEqual([]);
    expect(highlights.has(NOTE_DIFF_REMOVED_HIGHLIGHT)).toBe(false);
  });
});
