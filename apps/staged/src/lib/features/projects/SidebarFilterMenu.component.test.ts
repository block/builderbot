import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

const { filters } = vi.hoisted(() => ({ filters: { active: new Set<string>() } }));

vi.mock('./projectFilters.svelte', () => ({
  projectFiltersStore: {
    get activeFilters() {
      return filters.active;
    },
    get hasActiveFilters() {
      return filters.active.size > 0;
    },
    isFilterActive: (filter: string) => filters.active.has(filter),
    unreadCount: 2,
    runningCount: 1,
    repoFilters: [],
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

  it('is absent with no active filters', async () => {
    filters.active = new Set();
    const content = await openMenu();

    expect(rows(content)).toEqual(['Unread', 'Running']);
  });
});
