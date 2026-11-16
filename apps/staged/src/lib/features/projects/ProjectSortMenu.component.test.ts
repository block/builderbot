// Mounts the sort menu inside real bits-ui dropdown menus, over the real
// tagsStore, catalog overlay and sort preference, with the backend commands
// and the preferences file mocked: each edit is read off the menu and off
// what was saved under `projects-sort`.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TagState } from '../../types';
import { MOBILE_LAYOUT_QUERY } from '../../shared/viewport.svelte';
import { group, tagState } from '../tags/testStubs/tagFixtures';

const { backend, store } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  store: {
    getStoreValue: vi.fn(async (_key: string): Promise<unknown> => undefined),
    setStoreValue: vi.fn(async (_key: string, _value: unknown) => {}),
  },
}));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({ data: structuredClone(backend.state), revalidating: null })),
}));

vi.mock('../../commands', () => commandsMock);
vi.mock('../../api/commands', () => commandsMock);
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../shared/persistentStore', () => store);
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: { liveProjects: [], isProjectDeleting: () => false },
}));
vi.mock('../layout/navigation.svelte', () => ({ onViewHidden: () => {} }));
vi.mock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));

import { tagsStore } from '../../stores/tags.svelte';
import ProjectSortMenu from './ProjectSortMenu.svelte';
import type { ProjectSort, ProjectSortKey } from './projectSort';
import { PROJECT_SORT_STORE_KEY, projectSortPrefs } from './projectSortPrefs.svelte';

const created = (direction: 'asc' | 'desc' = 'desc'): ProjectSortKey => ({
  kind: 'created',
  direction,
});
const updated = (direction: 'asc' | 'desc' = 'desc'): ProjectSortKey => ({
  kind: 'updated',
  direction,
});
const byGroup = (groupId: string, direction: 'asc' | 'desc' = 'asc'): ProjectSortKey => ({
  kind: 'group',
  groupId,
  direction,
});

type MenuProps = { variant?: 'toolbar' | 'sidebar'; boardGroupId?: string | null };

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

async function loadTags(state: TagState) {
  backend.state = state;
  await tagsStore.load({ force: true });
  await settle();
}

/** Mount the menu over `sort`, with the fixture catalog loaded unless `state` is given. */
async function mountMenu(sort: ProjectSort = [], props: MenuProps = {}, state = tagState()) {
  await loadTags(state);
  projectSortPrefs.sort = sort;
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectSortMenu, { target, props });
  await settle();
}

function trigger(): HTMLButtonElement {
  return target.querySelector<HTMLButtonElement>('[aria-label^="Sort projects"]')!;
}

/** Open the menu from the keyboard and return its content. */
async function openMenu(): Promise<HTMLElement> {
  trigger().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await settle();
  return menu()!;
}

function menu(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-slot="dropdown-menu-content"]');
}

function subMenus(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('[data-slot="dropdown-menu-sub-content"]')];
}

/** The key rows top to bottom, as "number name hint". */
function keyRows(): string[] {
  return [...menu()!.querySelectorAll<HTMLElement>('[data-sort-key]')].map((row) =>
    [...row.querySelectorAll('span')].map((span) => span.textContent!.trim()).join(' | ')
  );
}

function keyRow(id: string): HTMLElement {
  const row = [...menu()!.querySelectorAll<HTMLElement>('[data-sort-key]')].find(
    (el) => el.dataset.sortKey === id
  );
  if (!row) throw new Error(`No row for ${id}`);
  return row;
}

function addRow(): HTMLElement | null {
  return menu()!.querySelector<HTMLElement>('[data-sort-add]');
}

/**
 * Open the submenu `row` leads to and return its content. A submenu closed
 * just before leaves the document a moment after it closes, so this waits
 * until the one asked for is the only one.
 */
async function openSub(row: HTMLElement): Promise<HTMLElement> {
  row.focus();
  row.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
  await settle();
  const id = row.getAttribute('aria-controls');
  await vi.waitFor(() => expect(subMenus().map((sub) => sub.id)).toEqual([id]));
  return subMenus()[0];
}

/** Every submenu has closed and left the document. */
async function subMenusClosed() {
  await vi.waitFor(() => expect(subMenus()).toEqual([]));
  await settle();
}

function items(content: ParentNode): HTMLElement[] {
  return [...content.querySelectorAll<HTMLElement>('[role^="menuitem"]')];
}

function labels(content: ParentNode): string[] {
  return items(content).map((el) => el.textContent!.trim());
}

function item(content: ParentNode, text: string): HTMLElement {
  const found = items(content).find((el) => el.textContent!.trim() === text);
  if (!found) throw new Error(`No menu item "${text}"`);
  return found;
}

async function pick(content: ParentNode, text: string) {
  const el = item(content, text);
  // As a keyboard pick has it: the item holds focus when it is selected.
  el.focus();
  el.click();
  await settle();
}

/** What was last saved as the window's sort, or undefined when nothing was. */
function saved(): unknown {
  const writes = store.setStoreValue.mock.calls.filter(([key]) => key === PROJECT_SORT_STORE_KEY);
  return writes.length === 0 ? undefined : writes[writes.length - 1][1];
}

/** Whether the window answers its media queries as a phone does: narrow and touch-driven. */
let phone = false;

beforeAll(() => {
  // jsdom lacks the layout APIs the menu's positioning and viewport use.
  window.matchMedia = ((query: string) => ({
    get matches() {
      return phone && (query === MOBILE_LAYOUT_QUERY || query === '(pointer: coarse)');
    },
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
  phone = false;
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  projectSortPrefs.sort = [];
  document.body.innerHTML = '';
});

describe('ProjectSortMenu before the tags are known', () => {
  it('waits, since an edit would take every group key for deleted', async () => {
    // First in the file: the store has no way back to unloaded.
    expect(tagsStore.loaded).toBe(false);
    projectSortPrefs.sort = [byGroup('priority')];
    target = document.createElement('div');
    document.body.appendChild(target);
    component = mount(ProjectSortMenu, { target });
    await settle();
    expect(trigger().disabled).toBe(true);

    // A first read that fails settles the tags as loaded, with the catalog
    // still empty: no group is known yet, so there is still nothing to edit
    // against.
    vi.spyOn(console, 'error').mockImplementation(() => {});
    commandsMock.getTagState.mockRejectedValueOnce(new Error('backend down'));
    await tagsStore.load();
    await settle();
    expect(tagsStore.loaded).toBe(true);
    expect(tagsStore.groups).toEqual([]);
    expect(trigger().disabled).toBe(true);

    await loadTags(tagState());
    expect(trigger().disabled).toBe(false);
    expect(trigger().textContent).toContain('Priority');
    expect(saved()).toBeUndefined();
  });
});

describe('ProjectSortMenu trigger', () => {
  it('reads "Sort" and "Newest" while the sort is the default', async () => {
    await mountMenu();
    expect(trigger().textContent!.replace(/\s+/g, ' ').trim()).toBe('Sort Newest');
    expect(trigger().getAttribute('aria-label')).toBe('Sort projects: Newest');
    expect(target.querySelector('.sort-dot')).toBeNull();
  });

  it('names the keys in order', async () => {
    await mountMenu([byGroup('priority'), updated()]);
    expect(trigger().textContent!.replace(/\s+/g, ' ').trim()).toBe('Sort Priority, Last updated');
    // Shown once the phone layout drops the name.
    expect(target.querySelector('.sort-dot')).not.toBeNull();
  });

  it('follows the sort as it is edited elsewhere', async () => {
    await mountMenu();
    projectSortPrefs.sort = [created('asc')];
    await settle();
    expect(trigger().textContent).toContain('Created');
  });

  it('leaves out a key for a group that no longer exists', async () => {
    await mountMenu([byGroup('gone'), updated()]);
    expect(trigger().getAttribute('aria-label')).toBe('Sort projects: Last updated');
  });

  it('is an icon in the sidebar, with a dot once the sort is not the default', async () => {
    await mountMenu([], { variant: 'sidebar' });
    expect(trigger().classList.contains('sidebar-sort-trigger')).toBe(true);
    expect(trigger().textContent!.trim()).toBe('');
    expect(trigger().getAttribute('title')).toBe('Sort projects');
    expect(target.querySelector('.sort-dot')).toBeNull();

    projectSortPrefs.sort = [byGroup('status')];
    await settle();
    expect(trigger().getAttribute('title')).toBe('Sort: Status');
    expect(trigger().getAttribute('aria-label')).toBe('Sort projects: Status');
    expect(target.querySelector('.sort-dot')).not.toBeNull();
  });

  it('opens the same menu from the sidebar', async () => {
    await mountMenu([updated()], { variant: 'sidebar' });
    await openMenu();
    expect(keyRows()).toEqual(['1 | Last updated | Most recent first']);
  });
});

describe('ProjectSortMenu with no keys', () => {
  it('says the order is the default and offers every key', async () => {
    await mountMenu();
    const content = await openMenu();

    expect(content.querySelector('[data-sort-default]')!.textContent!.trim()).toBe(
      'Newest first (default)'
    );
    expect(keyRows()).toEqual([]);
    expect(labels(content)).toEqual(['Add sort key']);

    const add = await openSub(addRow()!);
    // Created, last updated, then the groups in catalog order.
    expect(labels(add)).toEqual(['Created', 'Last updated', 'Status', 'Priority']);
  });
});

describe('ProjectSortMenu adding a key', () => {
  it('adds the key last, in its default direction, and saves the sort', async () => {
    await mountMenu([created()]);
    await openMenu();
    await pick(await openSub(addRow()!), 'Priority');

    expect(saved()).toEqual([created('desc'), byGroup('priority', 'asc')]);
    expect(keyRows()).toEqual(['1 | Created | Newest first', '2 | Priority | Tag order']);
    expect(trigger().textContent).toContain('Created, Priority');
  });

  it('keeps the menu open, closes the submenu, and focuses the new row', async () => {
    await mountMenu();
    await openMenu();
    await pick(await openSub(addRow()!), 'Last updated');

    await subMenusClosed();
    expect(menu()).not.toBeNull();
    expect(document.activeElement).toBe(keyRow('updated'));
    expect(menu()!.querySelector('[data-sort-default]')).toBeNull();
  });

  it('stops offering a key once it is in use, and Add once every key is', async () => {
    await mountMenu([created(), updated(), byGroup('status')]);
    await openMenu();
    const add = await openSub(addRow()!);
    expect(labels(add)).toEqual(['Priority']);

    await pick(add, 'Priority');
    expect(addRow()).toBeNull();
    expect(keyRows()).toHaveLength(4);
    expect(document.activeElement).toBe(keyRow('group:priority'));
  });

  it('offers the groups the catalog has now, in its order', async () => {
    const state = tagState();
    await mountMenu([], {}, state);
    // Priority moved above Status, and a group was added.
    await loadTags({
      ...state,
      groups: [
        ...state.groups.map((g) => ({ ...g, position: g.id === 'priority' ? 0 : 1 })),
        group('team', 2, { name: 'Team' }),
      ],
    });
    await openMenu();

    expect(labels(await openSub(addRow()!))).toEqual([
      'Created',
      'Last updated',
      'Priority',
      'Status',
      'Team',
    ]);
  });
});

describe('ProjectSortMenu editing a key', () => {
  it('words the directions for the kind of key, the default first', async () => {
    await mountMenu([created(), updated(), byGroup('status')]);
    await openMenu();

    const directions = async (id: string) => {
      const sub = await openSub(keyRow(id));
      return [...sub.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].map(
        (el) => `${el.textContent!.trim()}${el.getAttribute('aria-checked') === 'true' ? ' ✓' : ''}`
      );
    };
    expect(await directions('created')).toEqual(['Newest first ✓', 'Oldest first']);
    expect(await directions('updated')).toEqual(['Most recent first ✓', 'Least recent first']);
    expect(await directions('group:status')).toEqual(['Tag order ✓', 'Reverse tag order']);
  });

  it('flips a direction in place, leaving the submenu open', async () => {
    await mountMenu([byGroup('priority'), updated()]);
    await openMenu();
    const sub = await openSub(keyRow('group:priority'));
    await pick(sub, 'Reverse tag order');

    expect(saved()).toEqual([byGroup('priority', 'desc'), updated('desc')]);
    expect(keyRows()).toEqual([
      '1 | Priority | Reverse tag order',
      '2 | Last updated | Most recent first',
    ]);
    expect(subMenus()).toEqual([sub]);
    expect(item(sub, 'Reverse tag order').getAttribute('aria-checked')).toBe('true');

    // Picking the direction it already has writes nothing more.
    store.setStoreValue.mockClear();
    await pick(sub, 'Reverse tag order');
    expect(saved()).toBeUndefined();
  });

  it('moves a key up and down, and focus follows its row', async () => {
    await mountMenu([created(), updated(), byGroup('status')]);
    await openMenu();

    await pick(await openSub(keyRow('group:status')), 'Move up');
    expect(saved()).toEqual([created(), byGroup('status'), updated()]);
    expect(keyRows().map((row) => row.split(' | ').slice(0, 2).join(' '))).toEqual([
      '1 Created',
      '2 Status',
      '3 Last updated',
    ]);
    await subMenusClosed();
    expect(menu()).not.toBeNull();
    expect(document.activeElement).toBe(keyRow('group:status'));

    await pick(await openSub(keyRow('created')), 'Move down');
    expect(saved()).toEqual([byGroup('status'), created(), updated()]);
    expect(document.activeElement).toBe(keyRow('created'));
  });

  it('disables Move up on the first key and Move down on the last', async () => {
    await mountMenu([created(), updated()]);
    await openMenu();

    const disabled = (sub: ParentNode, text: string) =>
      item(sub, text).hasAttribute('data-disabled');
    const first = await openSub(keyRow('created'));
    expect(disabled(first, 'Move up')).toBe(true);
    expect(disabled(first, 'Move down')).toBe(false);

    const last = await openSub(keyRow('updated'));
    expect(disabled(last, 'Move up')).toBe(false);
    expect(disabled(last, 'Move down')).toBe(true);

    // A disabled move changes nothing.
    item(last, 'Move down').click();
    await settle();
    expect(saved()).toBeUndefined();
  });

  it('removes a key, and focus goes to the row that took its place', async () => {
    await mountMenu([created(), updated(), byGroup('status')]);
    await openMenu();

    await pick(await openSub(keyRow('updated')), 'Remove');
    expect(saved()).toEqual([created(), byGroup('status')]);
    expect(keyRows()).toEqual(['1 | Created | Newest first', '2 | Status | Tag order']);
    await subMenusClosed();
    expect(menu()).not.toBeNull();
    expect(document.activeElement).toBe(keyRow('group:status'));

    // Nothing follows the last key: the one before it takes focus.
    await pick(await openSub(keyRow('group:status')), 'Remove');
    expect(document.activeElement).toBe(keyRow('created'));

    // And with no key left, "Add sort key" does.
    await pick(await openSub(keyRow('created')), 'Remove');
    expect(saved()).toEqual([]);
    expect(document.activeElement).toBe(addRow());
    expect(menu()!.querySelector('[data-sort-default]')).not.toBeNull();
  });

  it('drops a key for a deleted group from the next write', async () => {
    await mountMenu([byGroup('gone'), created(), updated()]);
    await openMenu();
    // The dead key is not listed, so the numbers run from the first live key.
    expect(keyRows()).toEqual([
      '1 | Created | Newest first',
      '2 | Last updated | Most recent first',
    ]);
    expect(saved()).toBeUndefined();

    await pick(await openSub(keyRow('updated')), 'Move up');
    expect(saved()).toEqual([updated(), created()]);
  });
});

describe('ProjectSortMenu reset', () => {
  it('is offered only while the sort has a key', async () => {
    await mountMenu();
    expect(labels(await openMenu())).not.toContain('Reset to default');
  });

  it('clears the sort and closes the menu', async () => {
    await mountMenu([byGroup('priority'), updated()]);
    const content = await openMenu();
    expect(labels(content)).toContain('Reset to default');

    await pick(content, 'Reset to default');
    expect(saved()).toEqual([]);
    expect(projectSortPrefs.sort).toEqual([]);
    await vi.waitFor(() => expect(menu()).toBeNull());
    expect(trigger().textContent).toContain('Newest');
  });
});

describe('ProjectSortMenu on a board', () => {
  it("marks the key for the board's own group as doing nothing there", async () => {
    await mountMenu([byGroup('status', 'desc'), byGroup('priority'), updated()], {
      boardGroupId: 'status',
    });
    await openMenu();

    expect(keyRows()).toEqual([
      '1 | Status | Used for columns here',
      '2 | Priority | Tag order',
      '3 | Last updated | Most recent first',
    ]);
    expect(keyRow('group:status').hasAttribute('data-sort-inert')).toBe(true);
    expect(keyRow('group:priority').hasAttribute('data-sort-inert')).toBe(false);
    // The trigger still names it: it orders the grid and the sidebar.
    expect(trigger().textContent).toContain('Status, Priority, Last updated');
  });

  it('keeps the marked key editable', async () => {
    await mountMenu([byGroup('status'), updated()], { boardGroupId: 'status' });
    await openMenu();
    const sub = await openSub(keyRow('group:status'));
    await pick(sub, 'Reverse tag order');
    expect(saved()).toEqual([byGroup('status', 'desc'), updated()]);

    await pick(sub, 'Move down');
    expect(saved()).toEqual([updated(), byGroup('status', 'desc')]);
  });

  it('marks nothing off a board, or on a board by another group', async () => {
    await mountMenu([byGroup('status')]);
    await openMenu();
    expect(keyRows()).toEqual(['1 | Status | Tag order']);

    unmount(component!);
    component = null;
    document.body.innerHTML = '';
    await mountMenu([byGroup('status')], { boardGroupId: 'priority' });
    await openMenu();
    expect(keyRows()).toEqual(['1 | Status | Tag order']);
  });
});

describe('ProjectSortMenu as a phone sheet', () => {
  beforeEach(() => {
    phone = true;
  });

  /**
   * A tap: the pointer is in use, which bits-ui notes, and the item is
   * selected without holding keyboard focus.
   */
  async function tap(content: ParentNode, text: string) {
    const el = item(content, text);
    el.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    (document.activeElement as HTMLElement | null)?.blur();
    el.click();
    await settle();
  }

  it('returns to the menu after a tapped edit, with no row left reading as selected', async () => {
    await mountMenu([created()]);
    const content = await openMenu();
    await tap(await openSub(addRow()!), 'Priority');

    expect(saved()).toEqual([created(), byGroup('priority')]);
    await subMenusClosed();
    expect(menu()).toBe(content);
    expect(keyRows()).toEqual(['1 | Created | Newest first', '2 | Priority | Tag order']);
    // On the sheet itself, where the arrow keys still reach the first item.
    expect(document.activeElement).toBe(content);
  });

  it('focuses the row after an edit made from the keyboard', async () => {
    await mountMenu([created(), updated()]);
    await openMenu();
    await pick(await openSub(keyRow('updated')), 'Move up');

    expect(saved()).toEqual([updated(), created()]);
    await subMenusClosed();
    expect(document.activeElement).toBe(keyRow('updated'));
  });
});
