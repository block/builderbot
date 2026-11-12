// Mounts the real board over the real tagsStore, tagActions and board
// preferences, with the backend commands and the preference store mocked: a
// drop or a "Move to" ends in `set_project_group_tag` and moves the card at
// once, and a collapsed column is saved to `projects-board-columns`. The last
// block puts the real projectFiltersStore in front of it, as ProjectsList
// does, so the tag chips decide which cards it shows.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, TagState } from '../../types';
import { tagState, withGroupTag } from '../tags/testStubs/tagFixtures';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

const { backend, setStoreValue, openProject } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  setStoreValue: vi.fn(async (_key: string, _value: unknown) => {}),
  openProject: vi.fn(),
}));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({ data: structuredClone(backend.state), revalidating: null })),
  setProjectGroupTag: vi.fn(
    async (_projectId: string, _groupId: string, _tagId: string | null) => {}
  ),
  createTagGroup: vi.fn(async (name: string) => ({
    id: 'new-group',
    name,
    position: 0,
    isIconGroup: false,
    showInSidebar: false,
    createdAt: 1,
    updatedAt: 1,
  })),
}));

function project(id: string): Project {
  return {
    id,
    name: `Project ${id}`,
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt: 1,
    updatedAt: 1,
  };
}

const projects = vi.hoisted(() => [] as Project[]);

vi.mock('../../commands', () => commandsMock);
vi.mock('../../api/commands', () => commandsMock);
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../shared/persistentStore', () => ({
  getStoreValue: vi.fn(async () => undefined),
  setStoreValue,
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    get liveProjects() {
      return projects;
    },
    branchesByProject: new Map(),
    reposByProject: new Map(),
    deletingProjectNames: new Map(),
    transferringProjects: new Map(),
    isProjectDeleting: () => false,
    isProjectHydrated: () => true,
  },
}));
vi.mock('../projects/projectActions.svelte', () => ({
  projectActions: { markProjectUnread: vi.fn(), requestRemoveProject: vi.fn() },
}));
vi.mock('../projects/projectStatus', () => ({
  getProjectStatus: () => ({ kind: 'idle', runningCount: 0, runActionPhase: null }),
}));
vi.mock('../projects/workspaceLifecycle.svelte', () => ({
  workspaceLifecycle: { getWorktreeErrors: () => new Map() },
}));
vi.mock('../../stores/projectState.svelte', () => ({
  projectStateStore: { isUnread: () => false },
}));
vi.mock(
  '../projects/ProjectTransferMenuItem.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../projects/ProjectRepoBadges.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../projects/ProjectCardStatusIndicator.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../layout/navigation.svelte', () => ({ openSettings: vi.fn() }));
vi.mock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));

import { tagsStore } from '../../stores/tags.svelte';
import { projectFiltersStore } from '../projects/projectFilters.svelte';
import FilteredBoardHarness from './testStubs/FilteredBoardHarness.svelte';
import { PROJECT_DRAG_TYPE } from './boardDrop';
import { NO_TAG_COLUMN_KEY } from './boardPrefs';
import { boardViewPrefs } from './boardViewPrefs.svelte';
import ProjectsBoard from './ProjectsBoard.svelte';

let component: ReturnType<typeof mount> | null = null;

async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

/**
 * Mount the Status board (idea, doing, done) over p1 (Done), p2 (no status)
 * and p3 (Idea).
 */
async function mountBoard(
  state: TagState = tagState([
    { projectId: 'p1', tagId: 'done' },
    { projectId: 'p3', tagId: 'idea' },
  ]),
  { searchRank = null }: { searchRank?: Set<string> | null } = {}
) {
  backend.state = state;
  await tagsStore.load({ force: true });
  projects.splice(0, projects.length, project('p1'), project('p2'), project('p3'));
  const target = document.createElement('div');
  document.body.appendChild(target);
  const props = reactiveProps({
    group: tagsStore.catalog.groupsById.get('status') ?? null,
    projects: [...projects],
    searchRank,
    onOpen: openProject,
  });
  component = mount(ProjectsBoard, { target, props });
  await settle();
  return target;
}

function column(root: ParentNode, key: string): HTMLElement {
  const found = [...root.querySelectorAll<HTMLElement>('[data-board-column]')].find(
    (el) => el.dataset.boardColumn === key
  );
  if (!found) throw new Error(`No column ${key}`);
  return found;
}

function columnKeys(root: ParentNode): string[] {
  return [...root.querySelectorAll<HTMLElement>('[data-board-column]')].map(
    (el) => el.dataset.boardColumn!
  );
}

/** The cards a column shows (search misses excluded). */
function cardsIn(root: ParentNode, key: string): string[] {
  return [...column(root, key).querySelectorAll<HTMLElement>('[data-board-card]')]
    .filter((el) => !el.closest('[hidden]'))
    .map((el) => el.dataset.boardCard!);
}

function card(root: ParentNode, projectId: string): HTMLElement {
  const found = [...root.querySelectorAll<HTMLElement>('[data-board-card]')].find(
    (el) => el.dataset.boardCard === projectId
  );
  if (!found) throw new Error(`No card ${projectId}`);
  return found;
}

/** A drag event carrying a stub DataTransfer (jsdom has neither). */
function dragEvent(type: string, data: Record<string, string>): Event {
  const store = new Map(Object.entries(data));
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'dataTransfer', {
    value: {
      effectAllowed: 'all',
      dropEffect: 'none',
      get types() {
        return [...store.keys()];
      },
      setData: (key: string, value: string) => void store.set(key, value),
      getData: (key: string) => store.get(key) ?? '',
      setDragImage: () => {},
    },
  });
  return event;
}

function menuItem(content: ParentNode, text: string): HTMLElement {
  const found = [...content.querySelectorAll<HTMLElement>('[role^="menuitem"]')].find((el) =>
    el.textContent?.includes(text)
  );
  if (!found) throw new Error(`No menu item "${text}"`);
  return found;
}

/** Pick `tagName` from a card's "Move to" submenu. */
async function moveWithMenu(root: ParentNode, projectId: string, tagName: string) {
  card(root, projectId).dispatchEvent(
    new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })
  );
  await settle();
  const menu = document.querySelector<HTMLElement>('[data-slot="context-menu-content"]')!;
  const moveTo = menu.querySelector<HTMLElement>('[data-tag-group-trigger="move-to"]')!;
  moveTo.focus();
  moveTo.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
  await settle();
  const subs = document.querySelectorAll<HTMLElement>('[data-slot="context-menu-sub-content"]');
  menuItem(subs[subs.length - 1], tagName).click();
  await settle();
}

async function openColumnMenu(root: ParentNode, key: string): Promise<HTMLElement> {
  column(root, key)
    .querySelector<HTMLElement>('.board-column-menu-trigger')!
    .dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await settle();
  const menus = document.querySelectorAll<HTMLElement>('[data-slot="dropdown-menu-content"]');
  return menus[menus.length - 1];
}

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
  Element.prototype.scrollIntoView ??= () => {};
});

afterAll(() => {
  delete (window as Partial<Window>).matchMedia;
  delete (globalThis as Partial<typeof globalThis>).ResizeObserver;
});

beforeEach(() => {
  vi.clearAllMocks();
  boardViewPrefs.columns = {};
  boardViewPrefs.groupId = null;
  // The write changes what the next get_tag_state returns, as the backend's would.
  commandsMock.setProjectGroupTag.mockImplementation(async (projectId, groupId, tagId) => {
    backend.state = withGroupTag(backend.state, projectId, groupId, tagId);
  });
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('ProjectsBoard', () => {
  it('shows "No ‹group›" first, then a column per tag, each with its cards and count', async () => {
    const root = await mountBoard();

    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
    expect(column(root, NO_TAG_COLUMN_KEY).textContent).toContain('No Status');
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
    expect(cardsIn(root, 'idea')).toEqual(['p3']);
    expect(cardsIn(root, 'doing')).toEqual([]);
    expect(cardsIn(root, 'done')).toEqual(['p1']);
    expect(column(root, 'done').querySelector('.column-count')?.textContent).toBe('1');
    expect(column(root, 'doing').textContent).toContain('No projects');
  });

  it('hides the cards a search rules out, and counts only the rest', async () => {
    const root = await mountBoard(undefined, { searchRank: new Set(['p1']) });

    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual([]);
    expect(cardsIn(root, 'done')).toEqual(['p1']);
    // Still mounted, just hidden.
    expect(card(root, 'p2').closest('[hidden]')).not.toBeNull();
    expect(column(root, 'idea').querySelector('.column-count')?.textContent).toBe('0');
  });

  it('opens a project when its card is clicked', async () => {
    const root = await mountBoard();
    card(root, 'p3').click();
    expect(openProject).toHaveBeenCalledWith('p3');
  });

  it('moves a card dropped on another column, and clears it on "No ‹group›"', async () => {
    const root = await mountBoard();

    const doing = column(root, 'doing');
    const over = dragEvent('dragover', { [PROJECT_DRAG_TYPE]: 'p2', 'text/plain': 'p2' });
    doing.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
    doing.dispatchEvent(dragEvent('drop', { [PROJECT_DRAG_TYPE]: 'p2', 'text/plain': 'p2' }));
    await settle();

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p2']);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual([]);
    expect(document.activeElement).toBe(card(root, 'p2'));

    column(root, NO_TAG_COLUMN_KEY).dispatchEvent(dragEvent('drop', { [PROJECT_DRAG_TYPE]: 'p1' }));
    await settle();

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p1', 'status', null);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p1']);
  });

  it('ignores drags that aren’t cards', async () => {
    const root = await mountBoard();

    const over = dragEvent('dragover', { 'text/plain': 'hello' });
    column(root, 'doing').dispatchEvent(over);
    column(root, 'doing').dispatchEvent(dragEvent('drop', { 'text/plain': 'p2' }));
    await settle();

    expect(over.defaultPrevented).toBe(false);
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('moves a card with "Move to" in its context menu', async () => {
    const root = await mountBoard();

    card(root, 'p3').dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })
    );
    await settle();
    const menu = document.querySelector<HTMLElement>('[data-slot="context-menu-content"]')!;
    // On a board, changing column is the card's main action: "Move to" is
    // the first item, ahead of Mark as Unread and the tag submenus.
    const items = [...menu.querySelectorAll<HTMLElement>('[role^="menuitem"]')].map((el) =>
      el.textContent?.trim()
    );
    expect(items[0]).toContain('Move to');
    expect(items[1]).toContain('Mark as Unread');
    // "Move to" is the board group's submenu, so the Status submenu it would
    // repeat is left out; the other groups and Tags stay.
    const triggers = [...menu.querySelectorAll<HTMLElement>('[data-tag-group-trigger]')];
    expect(triggers.map((t) => t.dataset.tagGroupTrigger)).toEqual([
      'move-to',
      'priority',
      'ungrouped',
    ]);

    triggers[0].focus();
    triggers[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    await settle();
    const subs = document.querySelectorAll<HTMLElement>('[data-slot="context-menu-sub-content"]');
    const sub = subs[subs.length - 1];
    expect(menuItem(sub, 'No Status')).toBeTruthy();
    expect(menuItem(sub, 'Idea').getAttribute('aria-checked')).toBe('true');
    menuItem(sub, 'Doing').click();
    await settle();

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p3', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p3']);
    expect(cardsIn(root, 'idea')).toEqual([]);
    // Focus follows the card to its new column.
    expect(document.activeElement).toBe(card(root, 'p3'));
  });

  it('collapses a column to a strip, remembering it, and expands it again', async () => {
    const root = await mountBoard();

    menuItem(await openColumnMenu(root, 'done'), 'Collapse').click();
    await settle();

    expect(setStoreValue).toHaveBeenLastCalledWith('projects-board-columns', {
      status: { collapsed: ['done'], hidden: [] },
    });
    const strip = column(root, 'done').querySelector<HTMLButtonElement>('.collapsed-strip')!;
    expect(strip.textContent).toContain('Done');
    expect(strip.textContent).toContain('1');
    expect(cardsIn(root, 'done')).toEqual([]);

    strip.click();
    await settle();

    expect(setStoreValue).toHaveBeenLastCalledWith('projects-board-columns', {});
    expect(cardsIn(root, 'done')).toEqual(['p1']);
  });

  it('tints a collapsed column’s strip while a card is dragged over it', async () => {
    boardViewPrefs.columns = { status: { collapsed: ['done'], hidden: [] } };
    const root = await mountBoard();
    const done = column(root, 'done');
    const strip = done.querySelector<HTMLButtonElement>('.collapsed-strip')!;

    card(root, 'p3').dispatchEvent(dragEvent('dragstart', {}));
    done.dispatchEvent(dragEvent('dragover', { [PROJECT_DRAG_TYPE]: 'p3' }));
    await settle();

    expect(done.classList.contains('drop-target')).toBe(true);
    expect(strip.classList.contains('drop-target')).toBe(true);

    done.dispatchEvent(dragEvent('drop', { [PROJECT_DRAG_TYPE]: 'p3' }));
    await settle();

    expect(strip.classList.contains('drop-target')).toBe(false);
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p3', 'status', 'done');
    expect(strip.textContent).toContain('2');
  });

  it('marks each side with columns past it, and its chevron reveals the next one', async () => {
    const root = await mountBoard();
    const scroller = root.querySelector<HTMLElement>('.board-columns')!;
    const edges = () =>
      [...root.querySelectorAll<HTMLElement>('[data-board-scroll-edge]')].map(
        (el) => el.dataset.boardScrollEdge
      );
    // jsdom lays nothing out, so the columns fit.
    expect(edges()).toEqual([]);

    // An 800px row of five 272px columns (four and the + column), 12px apart.
    let scrollLeft = 0;
    Object.defineProperties(scroller, {
      clientWidth: { configurable: true, get: () => 800 },
      scrollWidth: { configurable: true, get: () => 1400 },
      scrollLeft: { configurable: true, get: () => scrollLeft },
    });
    const rect = (left: number, width: number) =>
      ({ left, right: left + width, top: 0, bottom: 300, width, height: 300 }) as DOMRect;
    scroller.getBoundingClientRect = () => rect(0, 800);
    [...scroller.children].forEach((child, i) => {
      child.getBoundingClientRect = () => rect(i * 284 - scrollLeft, 272);
    });
    const scrollBy = vi.fn();
    scroller.scrollBy = scrollBy as unknown as HTMLElement['scrollBy'];
    scroller.dispatchEvent(new Event('scroll'));
    await settle();
    expect(edges()).toEqual(['end']);

    // The third column ends at 840, under the right fade (from 752).
    root.querySelector<HTMLButtonElement>('[data-board-scroll-edge="end"] button')!.click();
    expect(scrollBy).toHaveBeenCalledWith({ left: 88, behavior: 'smooth' });

    scrollLeft = 300;
    scroller.dispatchEvent(new Event('scroll'));
    await settle();
    expect(edges()).toEqual(['start', 'end']);

    scrollLeft = 600;
    scroller.dispatchEvent(new Event('scroll'));
    await settle();
    expect(edges()).toEqual(['start']);
    // The third column now starts at -32, under the left fade (to 48).
    root.querySelector<HTMLButtonElement>('[data-board-scroll-edge="start"] button')!.click();
    expect(scrollBy).toHaveBeenLastCalledWith({ left: -80, behavior: 'smooth' });
  });

  it('hides a column from its menu, "No ‹group›" included', async () => {
    const root = await mountBoard();

    menuItem(await openColumnMenu(root, NO_TAG_COLUMN_KEY), 'Hide column').click();
    await settle();

    expect(columnKeys(root)).toEqual(['idea', 'doing', 'done']);
    expect(setStoreValue).toHaveBeenLastCalledWith('projects-board-columns', {
      status: { collapsed: [], hidden: [NO_TAG_COLUMN_KEY] },
    });
  });

  it('offers group templates when there are no groups, and boards by the one created', async () => {
    const root = await mountBoard({ groups: [], tags: [], assignments: [] });

    expect(root.textContent).toContain('No tag groups yet');
    const status = [...root.querySelectorAll<HTMLButtonElement>('button')].find((el) =>
      el.textContent?.includes('Status')
    )!;
    status.click();
    await settle();

    expect(commandsMock.createTagGroup).toHaveBeenCalledWith(
      'Status',
      expect.arrayContaining([expect.objectContaining({ name: 'Idea' })])
    );
    expect(setStoreValue).toHaveBeenCalledWith('projects-board-group', 'new-group');
  });
});

describe('ProjectsBoard under the tag filters', () => {
  /** The Status board behind the shared filters, over p1, p2 and p3. */
  async function mountFiltered(state: TagState) {
    backend.state = state;
    await tagsStore.load({ force: true });
    projects.splice(0, projects.length, project('p1'), project('p2'), project('p3'));
    const target = document.createElement('div');
    document.body.appendChild(target);
    component = mount(FilteredBoardHarness, {
      target,
      props: { group: tagsStore.catalog.groupsById.get('status') ?? null, onOpen: openProject },
    });
    await settle();
    return target;
  }

  afterEach(() => {
    projectFiltersStore.clearFilters();
  });

  it('shows only the cards the tag chips keep, and counts only them', async () => {
    const root = await mountFiltered(
      tagState([
        { projectId: 'p1', tagId: 'done' },
        { projectId: 'p1', tagId: 'q4' },
        { projectId: 'p2', tagId: 'q4' },
        { projectId: 'p3', tagId: 'idea' },
      ])
    );
    expect(cardsIn(root, 'idea')).toEqual(['p3']);

    projectFiltersStore.toggleFilter({ tagId: 'q4' });
    await settle();

    // Every column stays, to drop on; only q4's projects are in them.
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
    expect(cardsIn(root, 'done')).toEqual(['p1']);
    expect(cardsIn(root, 'idea')).toEqual([]);
    expect(column(root, 'idea').querySelector('.column-count')?.textContent).toBe('0');

    // A board-group chip ANDs with the ungrouped one.
    projectFiltersStore.toggleFilter({ tagId: 'done' }, { additive: true });
    await settle();
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual([]);
    expect(cardsIn(root, 'done')).toEqual(['p1']);
  });

  it('says when every match sits in a hidden column, and shows them on request', async () => {
    boardViewPrefs.columns = { status: { collapsed: [], hidden: ['done'] } };
    const root = await mountFiltered(
      tagState([
        { projectId: 'p1', tagId: 'done' },
        { projectId: 'p2', tagId: 'doing' },
        { projectId: 'p3', tagId: 'idea' },
      ])
    );
    expect(root.querySelector('.board-hint')).toBeNull();

    projectFiltersStore.toggleFilter({ tagId: 'done' });
    await settle();

    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing']);
    const hint = root.querySelector<HTMLElement>('.board-hint')!;
    expect(hint.textContent).toContain('The matching project is in a hidden column.');

    [...hint.querySelectorAll('button')]
      .find((el) => el.textContent?.includes('Show hidden'))!
      .click();
    await settle();

    expect(setStoreValue).toHaveBeenLastCalledWith('projects-board-columns', {});
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
    expect(cardsIn(root, 'done')).toEqual(['p1']);
    expect(root.querySelector('.board-hint')).toBeNull();
  });

  it('takes a card a move rules out off the board, and focuses its neighbour', async () => {
    const root = await mountFiltered(
      tagState([
        { projectId: 'p1', tagId: 'doing' },
        { projectId: 'p2', tagId: 'doing' },
        { projectId: 'p3', tagId: 'idea' },
      ])
    );
    projectFiltersStore.toggleFilter({ tagId: 'doing' });
    await settle();
    expect(cardsIn(root, 'doing')).toEqual(['p1', 'p2']);

    await moveWithMenu(root, 'p1', 'Done');

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p1', 'status', 'done');
    expect(cardsIn(root, 'doing')).toEqual(['p2']);
    expect(cardsIn(root, 'done')).toEqual([]);
    expect(document.activeElement).toBe(card(root, 'p2'));
  });
});
