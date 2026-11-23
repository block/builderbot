import { createRawSnippet, flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ParentBranchCommitsHover from './ParentBranchCommitsHover.svelte';

// The upstream-commits popover's two ways in: a mouse hovering the pill, as
// before, and a tap on a touch screen, where nothing hovers.

const listParentBranchCommits = vi.hoisted(() => vi.fn());
vi.mock('../../commands', () => ({ listParentBranchCommits }));
vi.mock('../../services/branchEventService', () => ({
  onBranchGitStateUpdated: () => () => {},
}));

const OPEN_DELAY_MS = 150;
const CLOSE_DELAY_MS = 120;

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

const anchor = () => target.querySelector<HTMLElement>('.hover-anchor')!;
const popover = () => target.querySelector<HTMLElement>('.commits-popover');

function pointer(node: EventTarget, type: string, pointerType: string): void {
  // pointerenter and pointerleave do not bubble; Svelte listens on the node.
  const bubbles = type !== 'pointerenter' && type !== 'pointerleave';
  const event = new MouseEvent(type, { bubbles });
  Object.defineProperty(event, 'pointerType', { value: pointerType });
  node.dispatchEvent(event);
  flushSync();
}

function hover(node: HTMLElement): void {
  pointer(node, 'pointerenter', 'mouse');
  node.dispatchEvent(new MouseEvent('mouseenter'));
  flushSync();
}

function unhover(node: HTMLElement): void {
  pointer(node, 'pointerleave', 'mouse');
  node.dispatchEvent(new MouseEvent('mouseleave'));
  flushSync();
}

function tap(node: HTMLElement): void {
  pointer(node, 'pointerenter', 'touch');
  pointer(node, 'pointerdown', 'touch');
  pointer(node, 'pointerup', 'touch');
  pointer(node, 'pointerleave', 'touch');
  // What iOS Safari makes up after a tap, ahead of the click.
  node.dispatchEvent(new MouseEvent('mouseenter'));
  node.click();
  flushSync();
}

function advance(ms: number): void {
  vi.advanceTimersByTime(ms);
  flushSync();
}

beforeEach(() => {
  vi.useFakeTimers();
  listParentBranchCommits.mockReset();
  listParentBranchCommits.mockResolvedValue([]);
  target = document.createElement('div');
  document.body.append(target);
  component = mount(ParentBranchCommitsHover, {
    target,
    props: {
      branchId: 'branch-1',
      baseBranch: 'main',
      count: 3,
      children: createRawSnippet(() => ({ render: () => '<span class="pill">main+3</span>' })),
    },
  });
  flushSync();
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  flushSync();
  document.body.innerHTML = '';
  vi.useRealTimers();
});

describe('mouse hover', () => {
  it('opens after the delay and closes after the pointer leaves', () => {
    hover(anchor());
    advance(OPEN_DELAY_MS - 1);
    expect(popover()).toBeNull();
    advance(1);
    expect(popover()).not.toBeNull();
    expect(listParentBranchCommits).toHaveBeenCalledWith('branch-1');

    unhover(anchor());
    advance(CLOSE_DELAY_MS - 1);
    expect(popover()).not.toBeNull();
    advance(1);
    expect(popover()).toBeNull();
  });

  it('stays open while the pointer is over the popover', () => {
    hover(anchor());
    advance(OPEN_DELAY_MS);
    unhover(anchor());
    popover()!.dispatchEvent(new MouseEvent('mouseenter'));
    advance(CLOSE_DELAY_MS * 2);
    expect(popover()).not.toBeNull();
  });

  it('does not open on a click', () => {
    pointer(anchor(), 'pointerdown', 'mouse');
    anchor().click();
    flushSync();
    expect(popover()).toBeNull();
  });

  it('still opens after an earlier touch', () => {
    tap(anchor());
    tap(anchor());
    hover(anchor());
    advance(OPEN_DELAY_MS);
    expect(popover()).not.toBeNull();
  });

  it('is not shut by a press elsewhere', () => {
    hover(anchor());
    advance(OPEN_DELAY_MS);
    pointer(document.body, 'pointerdown', 'mouse');
    expect(popover()).not.toBeNull();
  });
});

describe('touch', () => {
  it('opens at once on a tap', () => {
    tap(anchor());
    expect(popover()).not.toBeNull();
    expect(listParentBranchCommits).toHaveBeenCalledWith('branch-1');
    // No hover timer was left to close or reopen it.
    advance(OPEN_DELAY_MS + CLOSE_DELAY_MS);
    expect(popover()).not.toBeNull();
  });

  it('closes on a second tap', () => {
    tap(anchor());
    tap(anchor());
    expect(popover()).toBeNull();
  });

  it('closes on a press outside, and not on one inside the popover', () => {
    tap(anchor());
    pointer(popover()!, 'pointerdown', 'touch');
    expect(popover()).not.toBeNull();
    pointer(document.body, 'pointerdown', 'touch');
    expect(popover()).toBeNull();
  });

  it('closes on Escape', () => {
    tap(anchor());
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    flushSync();
    expect(popover()).toBeNull();
  });

  it('does not open for the mouse events made up after a touch that did not click', () => {
    pointer(anchor(), 'pointerenter', 'touch');
    pointer(anchor(), 'pointerdown', 'touch');
    anchor().dispatchEvent(new MouseEvent('mouseenter'));
    advance(OPEN_DELAY_MS * 2);
    expect(popover()).toBeNull();
  });

  it('is not closed by the mouse leave made up for the next tap elsewhere', () => {
    tap(anchor());
    anchor().dispatchEvent(new MouseEvent('mouseleave'));
    advance(CLOSE_DELAY_MS * 2);
    expect(popover()).not.toBeNull();
  });

  it('stops listening for outside presses once closed', () => {
    const remove = vi.spyOn(document, 'removeEventListener');
    tap(anchor());
    tap(anchor());
    expect(remove.mock.calls.map(([type]) => type)).toEqual(
      expect.arrayContaining(['pointerdown', 'keydown'])
    );
    remove.mockRestore();
  });
});
