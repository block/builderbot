import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import NoteDiffConnectors from './NoteDiffConnectors.svelte';
import {
  CONNECTOR_PREVIOUS_REACH,
  CONNECTOR_WIDTH,
  layoutNoteDiffConnectors,
} from './noteDiffConnectors';
import { NoteDiffState } from './noteDiffState.svelte';
import { extractNoteDiffUnits, measureNoteDiffUnits } from './noteDiffUnits';
import { renderNoteMarkdown } from './noteMarkdown';
import {
  BLOCK_HEIGHT,
  CURRENT_NOTE,
  HUNK_KINDS,
  layOut,
  pointerArrives,
  PREVIOUS_NOTE,
} from './testStubs/noteDiffFixtures';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

/**
 * The bands between the panes, over a diff whose two versions are measured
 * as the panes leave them: six blocks of 100px each, in panes 300px tall.
 * The previous version has a paragraph the note lacks (its third block), a
 * reworded one, and lacks the note's last.
 */

const VIEWPORT = 300;

let component: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  document.body.innerHTML = '';
  vi.spyOn(console, 'info').mockImplementation(() => {});
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  vi.restoreAllMocks();
});

/** One version rendered and laid out in a scroller of its own, and its blocks. */
function version(markdown: string) {
  const scroller = document.createElement('div');
  const content = document.createElement('div');
  content.innerHTML = renderNoteMarkdown(markdown);
  scroller.append(content);
  document.body.append(scroller);
  layOut(content, scroller, VIEWPORT);
  const units = extractNoteDiffUnits(content);
  return { units, boxes: measureNoteDiffUnits(units, scroller) };
}

function render({ height = VIEWPORT, measured = true } = {}) {
  const current = version(CURRENT_NOTE);
  const previous = version(PREVIOUS_NOTE);
  const noteDiff = new NoteDiffState();
  noteDiff.compare(renderNoteMarkdown(PREVIOUS_NOTE), current.units);
  noteDiff.setBoxes('current', current.boxes);
  if (measured) noteDiff.setBoxes('previous', previous.boxes);

  const onSelect = vi.fn<(hunkId: string) => void>();
  const onWheel = vi.fn<(deltaY: number) => void>();
  const props = reactiveProps({
    noteDiff,
    scrollTops: { previous: 0, current: 0 },
    height,
    onSelect,
    onWheel,
  });
  const target = document.createElement('div');
  document.body.append(target);
  component = mount(NoteDiffConnectors, { target, props });
  flushSync();
  return { props, noteDiff, onSelect, onWheel, previousBoxes: previous.boxes };
}

const svg = () => document.querySelector<SVGSVGElement>('.note-diff-connectors');
const bands = () => [...document.querySelectorAll<SVGGElement>('.note-diff-connector')];
const band = (kind: string) => bands().find((el) => el.dataset.kind === kind)!;
const fill = (el: Element) => el.querySelector<SVGPathElement>('.connector-fill')!;
const pointed = () => bands().filter((el) => el.classList.contains('is-pointed'));

/** A pointer event at a place in the window; a move bubbles there, as the browser's does. */
function pointer(type: string, el: Element, x: number, y: number) {
  el.dispatchEvent(
    new MouseEvent(type, { clientX: x, clientY: y, bubbles: type !== 'pointerenter' })
  );
  flushSync();
}

describe('NoteDiffConnectors', () => {
  it('draws one band per change in view, in the kind of its change', () => {
    // Panes tall enough to show every block of both versions.
    const { noteDiff } = render({ height: 7 * BLOCK_HEIGHT });

    expect(bands().map((el) => el.dataset.kind)).toEqual(HUNK_KINDS);
    expect(bands().map((el) => el.dataset.hunkId)).toEqual(noteDiff.hunks.map(({ id }) => id));
  });

  it('draws what the layout gives for the diff as it is measured and scrolled', () => {
    const { props, noteDiff } = render();
    props.scrollTops = { previous: 250, current: 150 };
    flushSync();

    const expected = layoutNoteDiffConnectors(noteDiff.hunks, noteDiff.boxes, {
      scrollTops: { previous: 250, current: 150 },
      width: CONNECTOR_WIDTH,
      height: VIEWPORT,
    });
    expect(expected.length).toBeGreaterThan(0);
    expect(bands().map((el) => fill(el).getAttribute('d'))).toEqual(
      expected.map((connector) => connector.fill)
    );
    expect(bands().map((el) => el.querySelector('.connector-stroke')!.getAttribute('d'))).toEqual(
      expected.map((connector) => connector.stroke)
    );
  });

  it('starts at the previous version’s markers and is as wide as the gutter', () => {
    render();

    expect(svg()!.style.left).toBe(`calc(100% - ${CONNECTOR_PREVIOUS_REACH}px)`);
    expect(svg()!.style.width).toBe(`${CONNECTOR_WIDTH}px`);
  });

  it('joins the removed paragraph in the previous version to the gap it left in the note', () => {
    render();

    // "M 0 top C … width gap C … 0 bottom Z": the third block on the left,
    // a point between the note's second and third blocks on the right.
    const d = fill(band('removed')).getAttribute('d')!;
    const numbers = d.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
    expect(numbers.slice(0, 2)).toEqual([0, 2 * BLOCK_HEIGHT + 0.5]);
    expect(numbers.slice(6, 8)).toEqual([CONNECTOR_WIDTH, 2 * BLOCK_HEIGHT]);
    expect(numbers.slice(-2)).toEqual([0, 3 * BLOCK_HEIGHT - 0.5]);
    expect(d).not.toContain('L');
  });

  it('leaves out a change scrolled out of both panes, and brings it back', () => {
    const { props } = render();
    // 300px tall at the top: the removal is in view, the changes below it are not.
    expect(bands().map((el) => el.dataset.kind)).toEqual(['removed']);

    props.scrollTops = { previous: 400, current: 300 };
    flushSync();
    expect(bands().map((el) => el.dataset.kind)).toEqual(['modified', 'added']);

    props.scrollTops = { previous: 0, current: 0 };
    flushSync();
    expect(bands().map((el) => el.dataset.kind)).toEqual(['removed']);
  });

  it('draws nothing until the previous version is measured', () => {
    const { noteDiff, previousBoxes } = render({ measured: false });
    expect(svg()).toBeNull();

    noteDiff.setBoxes('previous', previousBoxes);
    flushSync();
    expect(bands().length).toBeGreaterThan(0);
  });

  it('draws nothing with no change in view', () => {
    const { props } = render();

    // Both panes on their fourth block, which both versions hold unchanged.
    props.height = 50;
    props.scrollTops = { previous: 325, current: 225 };
    flushSync();

    expect(svg()).toBeNull();
  });

  it('lights the change while its band is hovered', () => {
    const { noteDiff } = render();
    pointer('pointermove', document.body, 640, 250);

    pointer('pointerenter', fill(band('removed')), 700, 250);
    expect(noteDiff.hoveredHunkId).toBe(noteDiff.hunks[0].id);
    expect(pointed().map((el) => el.dataset.kind)).toEqual(['removed']);

    fill(band('removed')).dispatchEvent(new MouseEvent('pointerleave'));
    flushSync();
    expect(noteDiff.hoveredHunkId).toBeNull();
    expect(pointed()).toEqual([]);
  });

  it('shows the selected change’s band as pointed at, and a hovered one over it', () => {
    const { noteDiff } = render({ height: 7 * BLOCK_HEIGHT });

    noteDiff.select(noteDiff.hunks[1].id, true);
    flushSync();
    expect(pointed().map((el) => el.dataset.kind)).toEqual(['modified']);

    pointer('pointermove', document.body, 640, 620);
    pointer('pointerenter', fill(band('added')), 700, 620);
    expect(pointed().map((el) => el.dataset.kind)).toEqual(['added']);

    fill(band('added')).dispatchEvent(new MouseEvent('pointerleave'));
    flushSync();
    expect(pointed().map((el) => el.dataset.kind)).toEqual(['modified']);
  });

  it('does not light a change whose band scrolls under a pointer at rest', () => {
    const { noteDiff } = render();
    // The pointer comes to rest in the gutter.
    pointer('pointermove', document.body, 700, 250);

    // A pane scrolls, and the browser reports the pointer, where it already
    // was, to the band now under it.
    pointer('pointerenter', fill(band('removed')), 700, 250);
    pointer('pointermove', fill(band('removed')), 700, 250);
    expect(noteDiff.hoveredHunkId).toBeNull();

    // The pointer moves on the band: that is a hover.
    pointer('pointermove', fill(band('removed')), 701, 250);
    expect(noteDiff.hoveredHunkId).toBe(noteDiff.hunks[0].id);
  });

  it('does not light a change whose band appears under a pointer it has not seen move', () => {
    // The bands are drawn when a key opens the previous version: the
    // pointer has been at rest since before they were there to watch it.
    const { noteDiff } = render();

    // The browser reports it, where it is, to the band now under it.
    pointer('pointerenter', fill(band('removed')), 700, 250);
    pointer('pointermove', fill(band('removed')), 700, 250);
    expect(noteDiff.hoveredHunkId).toBeNull();
    // And again when a pane scrolls.
    pointer('pointermove', fill(band('removed')), 700, 250);
    expect(noteDiff.hoveredHunkId).toBeNull();

    pointer('pointermove', fill(band('removed')), 701, 250);
    expect(noteDiff.hoveredHunkId).toBe(noteDiff.hunks[0].id);
  });

  it('lets go of a hovered change when its band scrolls out of the gutter', () => {
    const { props, noteDiff } = render();
    noteDiff.select(noteDiff.hunks[1].id, true);
    pointer('pointermove', document.body, 640, 250);
    pointer('pointerenter', fill(band('removed')), 700, 250);
    pointer('pointermove', fill(band('removed')), 700, 250);
    expect(noteDiff.pointedHunkId).toBe(noteDiff.hunks[0].id);

    // Both panes scroll past the removal. Its band is no longer drawn, and
    // an element that is removed is not left: no `pointerleave` comes.
    props.scrollTops = { previous: 400, current: 300 };
    flushSync();
    expect(bands().map((el) => el.dataset.kind)).toEqual(['modified', 'added']);

    expect(noteDiff.hoveredHunkId).toBeNull();
    // The selected change has its light back.
    expect(noteDiff.pointedHunkId).toBe(noteDiff.hunks[1].id);
    expect(pointed().map((el) => el.dataset.kind)).toEqual(['modified']);

    // Scrolled back under the pointer, which has not moved: not a hover.
    props.scrollTops = { previous: 0, current: 0 };
    flushSync();
    pointer('pointerenter', fill(band('removed')), 700, 250);
    expect(noteDiff.hoveredHunkId).toBeNull();
  });

  it('keeps a change hovered while its band is redrawn under the pointer', () => {
    const { props, noteDiff } = render();
    pointer('pointermove', document.body, 640, 250);
    pointer('pointerenter', fill(band('removed')), 700, 250);

    // A scroll that leaves the band in the gutter.
    props.scrollTops = { previous: 40, current: 40 };
    flushSync();

    expect(bands().map((el) => el.dataset.kind)).toContain('removed');
    expect(noteDiff.hoveredHunkId).toBe(noteDiff.hunks[0].id);
  });

  it('lets go of a hovered change when the bands are taken down', () => {
    const { noteDiff } = render();
    pointer('pointermove', document.body, 640, 250);
    pointer('pointerenter', fill(band('removed')), 700, 250);
    expect(noteDiff.hoveredHunkId).toBe(noteDiff.hunks[0].id);

    unmount(component!);
    component = null;

    expect(noteDiff.hoveredHunkId).toBeNull();
  });

  it('leaves a hover that is not its own when the bands are taken down', () => {
    const { noteDiff } = render();
    // A marker is hovered, not a band.
    noteDiff.hover(noteDiff.hunks[1].id);

    unmount(component!);
    component = null;

    expect(noteDiff.hoveredHunkId).toBe(noteDiff.hunks[1].id);
  });

  it('does not light a change for a touch, which has no hover to end', () => {
    const { noteDiff } = render();

    pointerArrives(fill(band('removed')), 'touch');
    flushSync();

    expect(noteDiff.hoveredHunkId).toBeNull();
  });

  it('reports a pressed band as its change', () => {
    const { noteDiff, onSelect } = render();

    fill(band('removed')).dispatchEvent(new MouseEvent('click', { bubbles: true }));

    expect(onSelect).toHaveBeenCalledExactlyOnceWith(noteDiff.hunks[0].id);
  });

  it('is for the pointer only: hidden from a screen reader, with nothing to tab to', () => {
    render({ height: 7 * BLOCK_HEIGHT });

    expect(svg()!.getAttribute('aria-hidden')).toBe('true');
    expect(svg()!.querySelectorAll('[tabindex], button, a, [role]')).toHaveLength(0);
  });

  it('hands the wheel on, in pixels', () => {
    const { onWheel } = render();

    fill(band('removed')).dispatchEvent(new WheelEvent('wheel', { deltaY: 40, bubbles: true }));
    expect(onWheel).toHaveBeenLastCalledWith(40);

    fill(band('removed')).dispatchEvent(
      new WheelEvent('wheel', { deltaY: 3, deltaMode: WheelEvent.DOM_DELTA_LINE, bubbles: true })
    );
    expect(onWheel).toHaveBeenLastCalledWith(48);
  });

  it('stops the note following its selected change when the wheel is turned over a band', () => {
    const { noteDiff } = render();
    noteDiff.select(noteDiff.hunks[0].id, true);
    flushSync();
    expect(noteDiff.following).toBe(true);

    fill(band('removed')).dispatchEvent(new WheelEvent('wheel', { deltaY: 40, bubbles: true }));

    expect(noteDiff.following).toBe(false);
  });
});
