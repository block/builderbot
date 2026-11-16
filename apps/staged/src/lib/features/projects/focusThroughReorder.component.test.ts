// Mounts a keyed list with keepFocusThroughReorder wired as the grid, the
// sidebar and the board wire it. jsdom drops focus from a node that is taken
// out of the document and put back, as browsers do, so a re-sort here loses
// focus the way it does on screen.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';
import ReorderFocusHarness from './testStubs/ReorderFocusHarness.svelte';

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

function mountList(ids: string[], guard = true) {
  target = document.createElement('div');
  document.body.appendChild(target);
  const props = reactiveProps({ ids, guard });
  component = mount(ReorderFocusHarness, { target, props });
  flushSync();
  return props;
}

function row(id: string): HTMLElement {
  const found = target.querySelector<HTMLElement>(`[data-row="${id}"]`);
  if (!found) throw new Error(`No row ${id}`);
  return found;
}

function order(): string[] {
  return [...target.querySelectorAll<HTMLElement>('[data-row]')].map((el) => el.dataset.row!);
}

/** The render has happened, and the focus check after it. */
async function rendered() {
  await tick();
  await Promise.resolve();
}

async function nextFrame() {
  await new Promise((resolve) => requestAnimationFrame(resolve));
}

afterEach(async () => {
  // Let the frame-later check run against the list it was for.
  await nextFrame();
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('keepFocusThroughReorder', () => {
  // Each order below moves the focused row itself: a keyed each keeps the
  // rows already in sequence and re-inserts the one that is out of place.
  it('a re-sort drops focus from the row it moves, without it', async () => {
    const props = mountList(['a', 'b', 'c', 'd'], false);
    row('a').focus();

    props.ids = ['b', 'c', 'd', 'a'];
    await rendered();
    await nextFrame();

    expect(order()).toEqual(['b', 'c', 'd', 'a']);
    expect(document.activeElement).toBe(document.body);
  });

  it('keeps focus on a row the re-sort moves', async () => {
    const props = mountList(['a', 'b', 'c', 'd']);
    const a = row('a');
    a.focus();

    props.ids = ['b', 'c', 'd', 'a'];
    await rendered();

    expect(order()).toEqual(['b', 'c', 'd', 'a']);
    // The same node, where the sort put it.
    expect(row('a')).toBe(a);
    expect(document.activeElement).toBe(a);
  });

  it('keeps it whichever way the row moves', async () => {
    const props = mountList(['a', 'b', 'c', 'd']);
    row('d').focus();

    props.ids = ['d', 'a', 'b', 'c'];
    await rendered();
    expect(document.activeElement).toBe(row('d'));

    props.ids = ['c', 'b', 'a', 'd'];
    await rendered();
    expect(order()).toEqual(['c', 'b', 'a', 'd']);
    expect(document.activeElement).toBe(row('d'));
  });

  it('takes focus back on the next frame if it dropped once the re-sort had rendered', async () => {
    const props = mountList(['a', 'b', 'c', 'd']);
    row('a').focus();
    props.ids = ['b', 'c', 'd', 'a'];
    await rendered();

    // WebKit can drop the focus a moved node was just given.
    row('a').blur();
    expect(document.activeElement).toBe(document.body);
    await nextFrame();
    expect(document.activeElement).toBe(row('a'));
  });

  it('goes to the row that took its place when the focused row left the list', async () => {
    const props = mountList(['a', 'b', 'c', 'd']);
    row('b').focus();

    // Removed in another window, or ruled out by a filter as its tags changed.
    props.ids = ['a', 'c', 'd'];
    await rendered();
    expect(document.activeElement).toBe(row('c'));

    // The last row goes: the closest one before it.
    row('d').focus();
    props.ids = ['a', 'c'];
    await rendered();
    expect(document.activeElement).toBe(row('c'));
  });

  it('leaves focus that is somewhere else where it is', async () => {
    const props = mountList(['a', 'b', 'c', 'd']);
    const outside = target.querySelector<HTMLElement>('[data-outside]')!;
    outside.focus();

    props.ids = ['d', 'c', 'b', 'a'];
    await rendered();
    await nextFrame();

    expect(document.activeElement).toBe(outside);
  });

  it('leaves focus the user moved after the re-sort where they put it', async () => {
    const props = mountList(['a', 'b', 'c', 'd']);
    row('a').focus();
    props.ids = ['b', 'c', 'd', 'a'];
    // Before the check runs, focus goes elsewhere.
    flushSync();
    const outside = target.querySelector<HTMLElement>('[data-outside]')!;
    outside.focus();
    await rendered();
    await nextFrame();

    expect(document.activeElement).toBe(outside);
  });

  it('focuses nothing when no row held focus', async () => {
    const props = mountList(['a', 'b', 'c', 'd']);

    props.ids = ['d', 'c', 'b', 'a'];
    await rendered();
    await nextFrame();

    expect(document.activeElement).toBe(document.body);
  });
});
