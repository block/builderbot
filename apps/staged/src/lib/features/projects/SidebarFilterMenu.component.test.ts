import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { group, tag } from '../tags/testStubs/tagFixtures';
import type { TagFilterSection } from './projectFilters.svelte';

const { filters } = vi.hoisted(() => ({
  filters: {
    active: new Set<string>(),
    /** The active set minus stale tag keys; the active set itself when null. */
    effective: null as Set<string> | null,
    sections: [] as TagFilterSection[],
  },
}));

vi.mock('./projectFilters.svelte', () => ({
  projectFiltersStore: {
    get activeFilters() {
      return filters.active;
    },
    get effectiveActiveFilters() {
      return filters.effective ?? filters.active;
    },
    get hasActiveFilters() {
      return (filters.effective ?? filters.active).size > 0;
    },
    isFilterActive: (filter: string | { tagId: string }) =>
      filters.active.has(typeof filter === 'string' ? filter : `tag:${filter.tagId}`),
    unreadCount: 2,
    runningCount: 1,
    repoFilters: [],
    get tagFilterSections() {
      return filters.sections;
    },
    toggleFilter: vi.fn(),
    clearFilters: vi.fn(),
  },
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: { loaded: false, ensureProjectsHydrated: vi.fn() },
}));
vi.mock('../../stores/repoBadges.svelte', () => ({ repoBadgeStore: { lookup: () => null } }));
vi.mock('../layout/navigation.svelte', () => ({ onViewHidden: () => {} }));

import SidebarFilterMenu from './SidebarFilterMenu.svelte';
import { projectFiltersStore } from './projectFilters.svelte';

let component: ReturnType<typeof mount> | null = null;

/** Mount the menu and open it from the keyboard, returning its content. */
async function openMenu(): Promise<HTMLElement> {
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(SidebarFilterMenu, { target });
  flushSync();
  const trigger = target.querySelector<HTMLElement>('[title="Filter projects"]')!;
  trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await settle();
  return document.querySelector<HTMLElement>('[data-slot="dropdown-menu-content"]')!;
}

/** Let positioning, the data-side observer and the re-render all land. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await tick();
  flushSync();
}

/** A row's first word, which drops the trailing counts. */
function label(el: Element | null): string {
  return (el?.textContent ?? '').trim().split(/\s/)[0];
}

/** The menu's rows top to bottom: item labels, with separators as '---'. */
function rows(content: HTMLElement): string[] {
  return [
    ...content.querySelectorAll('[role="menuitem"], [role="menuitemcheckbox"], [role="separator"]'),
  ].map((el) => (el.getAttribute('role') === 'separator' ? '---' : label(el)));
}

async function placeOn(content: HTMLElement, side: 'top' | 'bottom') {
  content.setAttribute('data-side', side);
  await settle();
}

/** Press a key on the focused item, the way the menu's roving focus sees it. */
function pressKey(key: string) {
  document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
}

/** bits-ui moves keyboard focus into an opened menu on the next frame. */
async function keyboardFocusLanded() {
  await new Promise((resolve) => requestAnimationFrame(resolve));
  await settle();
}

describe('SidebarFilterMenu Clear filters placement', () => {
  beforeAll(() => {
    // jsdom lacks the layout APIs the menu's positioning and viewport use.
    window.matchMedia = ((query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia;
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
  });

  afterAll(() => {
    delete (window as Partial<Window>).matchMedia;
    delete (globalThis as Partial<typeof globalThis>).ResizeObserver;
  });

  beforeEach(() => {
    filters.active = new Set(['unread']);
    filters.effective = null;
    filters.sections = [];
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('leads the list when the menu opens below the trigger', async () => {
    const content = await openMenu();
    await placeOn(content, 'bottom');

    expect(rows(content)).toEqual(['Clear', '---', 'Unread', 'Running']);
  });

  it('ends the list when the menu flips above the trigger', async () => {
    const content = await openMenu();
    await placeOn(content, 'top');

    expect(rows(content)).toEqual(['Unread', 'Running', '---', 'Clear']);
  });

  it('follows the menu when it flips while open', async () => {
    const content = await openMenu();
    await placeOn(content, 'top');
    await placeOn(content, 'bottom');

    expect(rows(content)).toEqual(['Clear', '---', 'Unread', 'Running']);
  });

  // bits-ui places the menu before its keyboard focus lands a frame later, so
  // a keyboard open starts on the first row of whichever order was settled.
  it('starts keyboard focus on Clear filters below the trigger', async () => {
    const content = await openMenu();
    await placeOn(content, 'bottom');
    await keyboardFocusLanded();

    expect(label(document.activeElement)).toBe('Clear');
  });

  it('starts keyboard focus on the first filter above the trigger', async () => {
    const content = await openMenu();
    await placeOn(content, 'top');
    await keyboardFocusLanded();

    expect(label(document.activeElement)).toBe('Unread');
  });

  // A flip swaps which branch renders Clear filters, replacing its node, so
  // focus on it has to be handed to the new node or keyboard input stops
  // reaching the menu.
  it('keeps focus on Clear filters when the menu flips above the trigger', async () => {
    const content = await openMenu();
    await placeOn(content, 'bottom');
    await keyboardFocusLanded();
    const before = document.activeElement;
    expect(label(before)).toBe('Clear');

    await placeOn(content, 'top');

    expect(rows(content)).toEqual(['Unread', 'Running', '---', 'Clear']);
    expect(document.activeElement).not.toBe(before);
    expect(content.contains(document.activeElement)).toBe(true);
    expect(label(document.activeElement)).toBe('Clear');
    expect(document.activeElement!.hasAttribute('data-highlighted')).toBe(true);
    // Arrow keys still rove from it, now upward into the filters.
    pressKey('ArrowUp');
    expect(label(document.activeElement)).toBe('Running');
  });

  it('keeps focus on Clear filters when the menu flips below the trigger', async () => {
    const content = await openMenu();
    await placeOn(content, 'top');
    await keyboardFocusLanded();
    expect(label(document.activeElement)).toBe('Unread');
    pressKey('ArrowUp'); // wraps to Clear filters, last above the trigger
    const before = document.activeElement;
    expect(label(before)).toBe('Clear');

    await placeOn(content, 'bottom');

    expect(rows(content)).toEqual(['Clear', '---', 'Unread', 'Running']);
    expect(document.activeElement).not.toBe(before);
    expect(content.contains(document.activeElement)).toBe(true);
    expect(label(document.activeElement)).toBe('Clear');
    expect(document.activeElement!.hasAttribute('data-highlighted')).toBe(true);
    pressKey('ArrowDown');
    expect(label(document.activeElement)).toBe('Unread');
  });

  it('leaves focus alone when the menu flips with another item focused', async () => {
    const content = await openMenu();
    await placeOn(content, 'bottom');
    await keyboardFocusLanded();
    pressKey('ArrowDown');
    pressKey('ArrowDown');
    const running = document.activeElement;
    expect(label(running)).toBe('Running');

    await placeOn(content, 'top');

    expect(document.activeElement).toBe(running);
  });

  it('is absent with no active filters', async () => {
    filters.active = new Set();
    const content = await openMenu();

    expect(rows(content)).toEqual(['Unread', 'Running']);
  });
});

describe('SidebarFilterMenu tags', () => {
  const status = group('status', 0, { name: 'Status', isIconGroup: true });

  beforeAll(() => {
    window.matchMedia = ((query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia;
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
  });

  afterAll(() => {
    delete (window as Partial<Window>).matchMedia;
    delete (globalThis as Partial<typeof globalThis>).ResizeObserver;
  });

  beforeEach(() => {
    vi.clearAllMocks();
    filters.active = new Set(['tag:q4']);
    filters.effective = null;
    filters.sections = [
      {
        key: 'group:status',
        label: 'Status',
        filters: [{ tag: tag('doing', 'status', 1, { name: 'Doing' }), group: status, count: 2 }],
      },
      {
        key: 'ungrouped',
        label: 'Tags',
        filters: [{ tag: tag('q4', null, 0, { name: 'q4' }), group: null, count: 1 }],
      },
    ];
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('lists the tags under a heading per group, then Tags, after the activity filters', async () => {
    const content = await openMenu();
    await placeOn(content, 'bottom');

    expect(rows(content)).toEqual([
      'Clear',
      '---',
      'Unread',
      'Running',
      '---',
      'Doing',
      '---',
      'q4',
    ]);
    const headings = [...content.querySelectorAll('[data-slot="dropdown-menu-label"]')].map((el) =>
      el.textContent?.trim()
    );
    // The built-in section isn't "Status": the default tag group is.
    expect(headings).toEqual(['Activity', 'Status', 'Tags']);
    const q4 = [...content.querySelectorAll('[role="menuitemcheckbox"]')].find(
      (el) => label(el) === 'q4'
    );
    expect(q4?.getAttribute('aria-checked')).toBe('true');
    expect(q4?.textContent).toContain('1');
  });

  it('toggles a tag additively, keeping the menu open', async () => {
    const content = await openMenu();
    const doing = [...content.querySelectorAll<HTMLElement>('[role="menuitemcheckbox"]')].find(
      (el) => label(el) === 'Doing'
    )!;
    doing.click();
    await settle();

    expect(projectFiltersStore.toggleFilter).toHaveBeenCalledWith(
      { tagId: 'doing' },
      { additive: true }
    );
    expect(document.querySelector('[data-slot="dropdown-menu-content"]')).not.toBeNull();
  });

  it('counts only the filters still in effect on its trigger', async () => {
    filters.active = new Set(['unread', 'tag:gone']);
    filters.effective = new Set(['unread']);
    const content = await openMenu();

    expect(document.querySelector('[title="Filter projects"]')?.textContent).toContain('1 filter');
    expect(rows(content)[0]).toBe('Clear');
  });

  it('hides Clear filters when only a deleted tag’s filter is left', async () => {
    filters.active = new Set(['tag:gone']);
    filters.effective = new Set();
    filters.sections = [];
    const content = await openMenu();

    expect(rows(content)).toEqual(['Unread', 'Running']);
    expect(document.querySelector('[title="Filter projects"]')?.textContent?.trim()).toBe('Filter');
  });
});
