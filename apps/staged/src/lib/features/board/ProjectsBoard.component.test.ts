// Mounts the real board over the real tagsStore, tagActions and board
// preferences, with the backend commands and the preference store mocked: a
// drop or a "Move to" ends in `set_project_group_tag` and moves the card at
// once, and a collapsed column is saved to `projects-board-columns`. A card's
// labels are the real ones, so its chips show which tags its column leaves
// out. Moving a column from its menu ends in `move_tag`, through the real
// pending overlay, so the board shows it before the write lands, and so
// does dragging one by its header. The last block puts the real projectFiltersStore in front of it, as
// ProjectsList does, so the tag chips decide which cards it shows.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, TagState } from '../../types';
import { withTagMoved } from '../tags/tagCatalogPatches';
import { scopeIds, tagState, withGroupTag } from '../tags/testStubs/tagFixtures';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';
import { viewport } from '../../shared/viewport.svelte';

const { backend, setStoreValue, openProject, toastError } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  setStoreValue: vi.fn(async (_key: string, _value: unknown) => {}),
  openProject: vi.fn(),
  toastError: vi.fn(),
}));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({ data: structuredClone(backend.state), revalidating: null })),
  setProjectGroupTag: vi.fn(
    async (_projectId: string, _groupId: string, _tagId: string | null) => {}
  ),
  moveTag: vi.fn(async (_tagId: string, _groupId: string | null, _index: number) => {}),
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
vi.mock('../../stores/repoBadges.svelte', () => ({ repoBadgeStore: { lookup: () => null } }));
vi.mock(
  '../projects/ProjectCardStatusIndicator.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../layout/navigation.svelte', () => ({ openSettings: vi.fn() }));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));

import { tagsStore } from '../../stores/tags.svelte';
import { projectFiltersStore } from '../projects/projectFilters.svelte';
import FilteredBoardHarness from './testStubs/FilteredBoardHarness.svelte';
import { COLUMN_DRAG_TYPE, PROJECT_DRAG_TYPE } from './boardDrop';
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
 * Mount a group's board, by default the Status board (idea, doing, done)
 * over p1 (Done), p2 (no status) and p3 (Idea).
 */
async function mountBoard(
  state: TagState = tagState([
    { projectId: 'p1', tagId: 'done' },
    { projectId: 'p3', tagId: 'idea' },
  ]),
  {
    searchRank = null,
    groupId = 'status',
  }: { searchRank?: Set<string> | null; groupId?: string } = {}
) {
  backend.state = state;
  await tagsStore.load({ force: true });
  projects.splice(0, projects.length, project('p1'), project('p2'), project('p3'));
  const target = document.createElement('div');
  document.body.appendChild(target);
  const props = reactiveProps({
    group: tagsStore.catalog.groupsById.get(groupId) ?? null,
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

/** The names on a card's tag chips, in order. */
function chipsOn(root: ParentNode, projectId: string): string[] {
  return [...card(root, projectId).querySelectorAll('.tag-chip-name')].map((el) =>
    el.textContent!.trim()
  );
}

/** Where a drag event's pointer is, and what a dragleave moved on to. */
interface DragPointer {
  clientX?: number;
  clientY?: number;
  relatedTarget?: EventTarget | null;
}

/**
 * A drag event carrying a stub DataTransfer (jsdom has neither), with the
 * pointer at `pointer`.
 */
function dragEvent(type: string, data: Record<string, string>, pointer: DragPointer = {}): Event {
  const store = new Map(Object.entries(data));
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, {
    clientX: { value: pointer.clientX ?? 0 },
    clientY: { value: pointer.clientY ?? 0 },
    relatedTarget: { value: pointer.relatedTarget ?? null },
  });
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

/** The types a drag event's DataTransfer carries. */
function dragTypes(event: Event): readonly string[] {
  return (event as DragEvent).dataTransfer!.types;
}

/**
 * Lay the row of columns out, which jsdom does not: 272px columns 12px
 * apart from x = 0, 300px tall, in the order they are in when asked.
 */
function layOutColumns(root: ParentNode): HTMLElement {
  const row = root.querySelector<HTMLElement>('.board-columns')!;
  const rect = (left: number, width: number) =>
    ({ left, right: left + width, top: 0, bottom: 300, width, height: 300 }) as DOMRect;
  row.getBoundingClientRect = () => rect(0, row.children.length * 284);
  for (const child of row.children) {
    child.getBoundingClientRect = () => rect([...row.children].indexOf(child) * 284, 272);
  }
  return row;
}

/** A pointer over the left or right half of a column (laid out by `layOutColumns`). */
function pointerOver(root: ParentNode, key: string, half: 'left' | 'right'): DragPointer {
  const rect = column(root, key).getBoundingClientRect();
  return { clientX: half === 'left' ? rect.left + 100 : rect.right - 100, clientY: 20 };
}

/** What a column drags by: its header, or its strip when collapsed. */
function columnHandle(root: ParentNode, key: string): HTMLElement {
  return column(root, key).querySelector<HTMLElement>('.board-column-head, .collapsed-strip')!;
}

/**
 * Pick column `key` up by its handle and hold it over a half of column
 * `onto`. Returns the dragover.
 */
function dragColumnOver(
  root: ParentNode,
  key: string,
  onto: string,
  half: 'left' | 'right'
): Event {
  columnHandle(root, key).dispatchEvent(dragEvent('dragstart', {}));
  const over = dragEvent('dragover', { [COLUMN_DRAG_TYPE]: key }, pointerOver(root, onto, half));
  column(root, onto).dispatchEvent(over);
  flushSync();
  return over;
}

/** Let go of column `key` over a half of column `onto`: the drop, then the source's dragend. */
async function dropColumnOn(root: ParentNode, key: string, onto: string, half: 'left' | 'right') {
  column(root, onto).dispatchEvent(
    dragEvent('drop', { [COLUMN_DRAG_TYPE]: key }, pointerOver(root, onto, half))
  );
  columnHandle(root, key).dispatchEvent(dragEvent('dragend', {}));
  await settle();
}

/** Drag column `key` by its handle and drop it on a half of column `onto`. */
async function dragColumn(root: ParentNode, key: string, onto: string, half: 'left' | 'right') {
  dragColumnOver(root, key, onto, half);
  await dropColumnOn(root, key, onto, half);
}

/** Each column showing a drop line, as `key:side`. */
function dropLines(root: ParentNode): string[] {
  return [...root.querySelectorAll<HTMLElement>('[data-board-column]')].flatMap((el) =>
    ['before', 'after']
      .filter((side) => el.classList.contains(`drop-${side}`))
      .map((side) => `${el.dataset.boardColumn}:${side}`)
  );
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

function columnMenuTrigger(root: ParentNode, key: string): HTMLElement {
  return column(root, key).querySelector<HTMLElement>('.board-column-menu-trigger')!;
}

/** Pick "Move left" or "Move right" from a column's menu. */
async function moveColumn(root: ParentNode, key: string, item: 'Move left' | 'Move right') {
  menuItem(await openColumnMenu(root, key), item).click();
  await settle();
}

/** Whether a column's menu offers `item`, and whether it can be picked. */
async function columnMenuItem(
  root: ParentNode,
  key: string,
  item: string
): Promise<'enabled' | 'disabled' | 'absent'> {
  const content = await openColumnMenu(root, key);
  const found = [...content.querySelectorAll<HTMLElement>('[role^="menuitem"]')].find((el) =>
    el.textContent?.includes(item)
  );
  const state = !found ? 'absent' : found.hasAttribute('data-disabled') ? 'disabled' : 'enabled';
  content.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await settle();
  return state;
}

/**
 * Settles each `move_tag` a test held open. The pending overlay and the
 * catalog write chain are module-scoped, so one left open (by a failed
 * assertion, say) would hold up every later test's moves; afterEach runs
 * whatever the test didn't.
 */
const openMoves = new Set<() => void>();

/**
 * Hold every `move_tag` open. Each entry of the result lands one, in call
 * order, as the backend would.
 */
function holdMoves(): (() => void)[] {
  const finishes: (() => void)[] = [];
  commandsMock.moveTag.mockImplementation(
    (tagId, groupId, index) =>
      new Promise<void>((resolve) => {
        const finish = () => {
          if (!openMoves.delete(finish)) return;
          backend.state = withTagMoved(backend.state, { tagId, groupId, index });
          resolve();
        };
        openMoves.add(finish);
        finishes.push(finish);
      })
  );
  return finishes;
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
  commandsMock.moveTag.mockImplementation(async (tagId, groupId, index) => {
    backend.state = withTagMoved(backend.state, { tagId, groupId, index });
  });
});

afterEach(async () => {
  while (openMoves.size > 0) {
    for (const settleMove of [...openMoves]) settleMove();
    await settle();
  }
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

  it('leaves the tag a card’s column stands for off the card, and keeps its other tags', async () => {
    // The Priority board: p1 under P0, p2 under P1, p3 under "No Priority".
    const root = await mountBoard(
      tagState([
        { projectId: 'p1', tagId: 'p0' },
        { projectId: 'p1', tagId: 'q4' },
        { projectId: 'p1', tagId: 'infra' },
        { projectId: 'p2', tagId: 'p1' },
        { projectId: 'p3', tagId: 'q4' },
      ]),
      { groupId: 'priority' }
    );

    expect(cardsIn(root, 'p0')).toEqual(['p1']);
    expect(chipsOn(root, 'p1')).toEqual(['q4', 'infra']);
    // Nothing but the column's tag: no chips at all.
    expect(cardsIn(root, 'p1')).toEqual(['p2']);
    expect(chipsOn(root, 'p2')).toEqual([]);
    // "No ‹group›" stands for no tag, so its cards show every chip.
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p3']);
    expect(chipsOn(root, 'p3')).toEqual(['q4']);
  });

  it('leaves out the new column’s tag once a card moves, not the old one’s', async () => {
    const root = await mountBoard(
      tagState([
        { projectId: 'p1', tagId: 'p0' },
        { projectId: 'p1', tagId: 'q4' },
      ]),
      { groupId: 'priority' }
    );

    expect(chipsOn(root, 'p1')).toEqual(['q4']);

    await moveWithMenu(root, 'p1', 'P1');

    expect(cardsIn(root, 'p1')).toEqual(['p1']);
    expect(chipsOn(root, 'p1')).toEqual(['q4']);
  });

  it('keeps another group’s tag on the card: only the board’s own group is implied', async () => {
    // On the Status board a card's Priority tag is not what its column says.
    const root = await mountBoard(
      tagState([
        { projectId: 'p1', tagId: 'done' },
        { projectId: 'p1', tagId: 'p0' },
        { projectId: 'p2', tagId: 'p1' },
      ])
    );

    expect(cardsIn(root, 'done')).toEqual(['p1']);
    expect(chipsOn(root, 'p1')).toEqual(['P0']);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2', 'p3']);
    expect(chipsOn(root, 'p2')).toEqual(['P1']);
    expect(chipsOn(root, 'p3')).toEqual([]);
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

describe('ProjectsBoard column moves', () => {
  it('moves a column right from its menu, showing it before the write lands', async () => {
    const finishes = holdMoves();
    const root = await mountBoard();

    await moveColumn(root, 'idea', 'Move right');

    // The write is still open, and the board already shows the new order.
    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);
    expect(scopeIds(backend.state, 'status')).toEqual(['idea', 'doing', 'done']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);
    // The cards went with their column.
    expect(cardsIn(root, 'idea')).toEqual(['p3']);

    finishes[0]();
    await settle();

    expect(scopeIds(backend.state, 'status')).toEqual(['doing', 'idea', 'done']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);
    expect(commandsMock.moveTag).toHaveBeenCalledTimes(1);
  });

  it('moves a column left from its menu', async () => {
    const root = await mountBoard();

    await moveColumn(root, 'done', 'Move left');

    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 1]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'done', 'doing']);
  });

  it.each([
    ['idea', 'Move right'],
    ['doing', 'Move left'],
  ] as const)('returns focus to %s’s menu trigger after "%s"', async (key, item) => {
    const root = await mountBoard();

    await moveColumn(root, key, item);

    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);
    expect(document.activeElement).toBe(columnMenuTrigger(root, key));
  });

  it('takes focus back on the next frame if it dropped once the move had rendered', async () => {
    const root = await mountBoard();
    const content = await openColumnMenu(root, 'idea');
    // Hold the frames from here on, the board's recheck among them.
    const frames: FrameRequestCallback[] = [];
    const raf = vi
      .spyOn(window, 'requestAnimationFrame')
      .mockImplementation((callback) => frames.push(callback));
    try {
      menuItem(content, 'Move right').click();
      await settle();
      const trigger = columnMenuTrigger(root, 'idea');
      expect(document.activeElement).toBe(trigger);

      // As when the menu closes late and WebKit drops focus from the column.
      trigger.blur();
      expect(document.activeElement).toBe(document.body);
      for (const frame of frames.splice(0)) frame(0);
      expect(document.activeElement).toBe(trigger);
    } finally {
      raf.mockRestore();
    }
  });

  it('leaves focus the user has since put elsewhere where it is', async () => {
    const root = await mountBoard();
    const content = await openColumnMenu(root, 'idea');
    const frames: FrameRequestCallback[] = [];
    const raf = vi
      .spyOn(window, 'requestAnimationFrame')
      .mockImplementation((callback) => frames.push(callback));
    try {
      menuItem(content, 'Move right').click();
      await settle();

      card(root, 'p1').focus();
      for (const frame of frames.splice(0)) frame(0);
      expect(document.activeElement).toBe(card(root, 'p1'));
    } finally {
      raf.mockRestore();
    }
  });

  it('turns a move off at the ends, and offers none on "No ‹group›"', async () => {
    const root = await mountBoard();

    // "No Status" is not a tag: it stays first, and the first tag column
    // can't move past it.
    expect(await columnMenuItem(root, NO_TAG_COLUMN_KEY, 'Move left')).toBe('absent');
    expect(await columnMenuItem(root, NO_TAG_COLUMN_KEY, 'Move right')).toBe('absent');
    expect(await columnMenuItem(root, 'idea', 'Move left')).toBe('disabled');
    expect(await columnMenuItem(root, 'idea', 'Move right')).toBe('enabled');
    expect(await columnMenuItem(root, 'doing', 'Move left')).toBe('enabled');
    expect(await columnMenuItem(root, 'doing', 'Move right')).toBe('enabled');
    expect(await columnMenuItem(root, 'done', 'Move left')).toBe('enabled');
    expect(await columnMenuItem(root, 'done', 'Move right')).toBe('disabled');

    // Picking a move that is off sends nothing.
    await moveColumn(root, 'idea', 'Move left');
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
  });

  it('follows the order a move in flight shows when turning moves off', async () => {
    holdMoves();
    const root = await mountBoard();

    await moveColumn(root, 'doing', 'Move right');

    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'done', 'doing']);
    expect(await columnMenuItem(root, 'doing', 'Move right')).toBe('disabled');
    expect(await columnMenuItem(root, 'done', 'Move right')).toBe('enabled');
  });

  it('steps over a hidden column, taking the move past it', async () => {
    boardViewPrefs.columns = { status: { collapsed: [], hidden: ['doing'] } };
    const root = await mountBoard();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'done']);

    await moveColumn(root, 'idea', 'Move right');

    // Past Done, the next column showing, not just past the hidden Doing.
    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
    expect(scopeIds(backend.state, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea']);
  });

  it('counts only showing columns as an end', async () => {
    boardViewPrefs.columns = { status: { collapsed: [], hidden: ['idea', 'done'] } };
    const root = await mountBoard();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing']);

    // Idea and Done sit either side of Doing in the stored order, hidden.
    expect(await columnMenuItem(root, 'doing', 'Move left')).toBe('disabled');
    expect(await columnMenuItem(root, 'doing', 'Move right')).toBe('disabled');
  });

  it('steps over a collapsed column as over any showing one', async () => {
    boardViewPrefs.columns = { status: { collapsed: ['doing'], hidden: [] } };
    const root = await mountBoard();

    await moveColumn(root, 'idea', 'Move right');

    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);
  });

  it('queues quick presses, each from where the last left the column', async () => {
    const finishes = holdMoves();
    const root = await mountBoard();

    await moveColumn(root, 'idea', 'Move right');
    await moveColumn(root, 'idea', 'Move right');

    // Both show at once, and the backend sees them one at a time, in order.
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);

    finishes[0]();
    await settle();
    expect(commandsMock.moveTag.mock.calls).toEqual([
      ['idea', 'status', 1],
      ['idea', 'status', 2],
    ]);
    // The first write landing doesn't throw the second's move away.
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);

    finishes[1]();
    await settle();
    expect(scopeIds(backend.state, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
  });

  it('resolves a move against the order the moves in flight show', async () => {
    const finishes = holdMoves();
    const root = await mountBoard();

    await moveColumn(root, 'idea', 'Move right');
    await moveColumn(root, 'done', 'Move left');

    // Done steps past Idea, its neighbour as shown, to second. Against the
    // stored order its neighbour is Doing, and it would land first.
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);

    finishes[0]();
    await settle();
    expect(commandsMock.moveTag.mock.calls).toEqual([
      ['idea', 'status', 1],
      ['done', 'status', 1],
    ]);

    finishes[1]();
    await settle();
    expect(scopeIds(backend.state, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
  });

  it('puts the columns back when the write fails', async () => {
    let fail: (error: Error) => void = () => {};
    commandsMock.moveTag.mockImplementation(
      () =>
        new Promise<void>((_resolve, reject) => {
          fail = (error) => {
            openMoves.delete(unsettled);
            reject(error);
          };
          const unsettled = () => fail(new Error('left open'));
          openMoves.add(unsettled);
        })
    );
    const root = await mountBoard();

    await moveColumn(root, 'idea', 'Move right');
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);

    fail(new Error('database is locked'));
    await settle();

    expect(toastError).toHaveBeenCalledWith('Unable to move tag', {
      description: 'database is locked',
    });
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
  });

  it('scrolls a moved column back clear of the edge fade', async () => {
    const root = await mountBoard();
    const scroller = root.querySelector<HTMLElement>('.board-columns')!;
    // An 800px row of 272px columns 12px apart, laid out in DOM order: Doing,
    // the third, ends at 840, under the right fade (from 752).
    let scrollLeft = 0;
    Object.defineProperty(scroller, 'scrollLeft', {
      configurable: true,
      get: () => scrollLeft,
      set: (value: number) => (scrollLeft = value),
    });
    const rect = (left: number, width: number) =>
      ({ left, right: left + width, top: 0, bottom: 300, width, height: 300 }) as DOMRect;
    scroller.getBoundingClientRect = () => rect(0, 800);
    for (const child of scroller.children) {
      child.getBoundingClientRect = () =>
        rect([...scroller.children].indexOf(child) * 284 - scrollLeft, 272);
    }

    // Idea, the second column, is in full view where it starts.
    await moveColumn(root, 'idea', 'Move right');

    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);
    expect(scrollLeft).toBe(840 - 752);

    // Back to second, it is in view again: the row stays where it is.
    await moveColumn(root, 'idea', 'Move left');
    expect(scrollLeft).toBe(840 - 752);
  });
});

describe('ProjectsBoard column drag', () => {
  it('drops a column after the one whose right half it lands on, before the write lands', async () => {
    const finishes = holdMoves();
    const root = await mountBoard();
    layOutColumns(root);

    const start = dragEvent('dragstart', {});
    columnHandle(root, 'idea').dispatchEvent(start);
    // Its own type only: not a card, and nothing to paste into a text field.
    expect(dragTypes(start)).toEqual([COLUMN_DRAG_TYPE]);

    const over = dragColumnOver(root, 'idea', 'done', 'right');
    expect(over.defaultPrevented).toBe(true);
    expect(dropLines(root)).toEqual(['done:after']);
    expect(column(root, 'idea').classList.contains('dragging')).toBe(true);

    await dropColumnOn(root, 'idea', 'done', 'right');

    // The write is still open, and the board already shows the new order.
    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
    expect(scopeIds(backend.state, 'status')).toEqual(['idea', 'doing', 'done']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
    expect(cardsIn(root, 'idea')).toEqual(['p3']);
    expect(dropLines(root)).toEqual([]);
    expect(column(root, 'idea').classList.contains('dragging')).toBe(false);
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();

    finishes[0]();
    await settle();

    expect(scopeIds(backend.state, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
    expect(commandsMock.moveTag).toHaveBeenCalledTimes(1);
  });

  it('drops a column before the one whose left half it lands on', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'done', 'idea', 'left');
    expect(dropLines(root)).toEqual(['idea:before']);
    // An engine that drops the drag's data: the board saw which column it was.
    column(root, 'idea').dispatchEvent(dragEvent('drop', {}, pointerOver(root, 'idea', 'left')));
    await settle();

    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
  });

  it.each(['left', 'right'] as const)(
    'puts a column dropped on the %s half of "No ‹group›" first, after it',
    async (half) => {
      const root = await mountBoard();
      layOutColumns(root);

      dragColumnOver(root, 'done', NO_TAG_COLUMN_KEY, half);
      expect(dropLines(root)).toEqual([`${NO_TAG_COLUMN_KEY}:after`]);
      await dropColumnOn(root, 'done', NO_TAG_COLUMN_KEY, half);

      expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
      expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
    }
  );

  it.each([
    ['on itself', 'doing', 'doing', 'left'],
    ['on the near side of the column after it', 'idea', 'doing', 'left'],
    ['on the near side of the column before it', 'doing', 'idea', 'right'],
    ['on "No ‹group›" when already first', 'idea', NO_TAG_COLUMN_KEY, 'right'],
  ] as const)('shows no line and writes nothing for a drop %s', async (_case, key, onto, half) => {
    const root = await mountBoard();
    layOutColumns(root);

    const over = dragColumnOver(root, key, onto, half);

    // Still a drop the board takes, so the pointer doesn't flicker to
    // "not allowed" on the way past.
    expect(over.defaultPrevented).toBe(true);
    expect(dropLines(root)).toEqual([]);

    await dropColumnOn(root, key, onto, half);

    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
  });

  it.each([
    ['names the row it moved on to', (row: HTMLElement): DragPointer => ({ relatedTarget: row })],
    // Past Doing's right edge (840), short of Done's left (852), in the row.
    ['names nothing, the pointer in the gap', (): DragPointer => ({ clientX: 846, clientY: 20 })],
  ] as const)(
    'takes a drop in the gap where the last column put the line, when the dragleave %s',
    async (_case, leaveTo) => {
      const root = await mountBoard();
      const row = layOutColumns(root);

      dragColumnOver(root, 'idea', 'doing', 'right');
      expect(dropLines(root)).toEqual(['doing:after']);

      // Off Doing and into the gap, where the line is drawn.
      column(root, 'doing').dispatchEvent(dragEvent('dragleave', {}, leaveTo(row)));
      const over = dragEvent('dragover', { [COLUMN_DRAG_TYPE]: 'idea' }, { clientX: 846 });
      row.dispatchEvent(over);
      flushSync();

      expect(over.defaultPrevented).toBe(true);
      expect(dropLines(root)).toEqual(['doing:after']);

      row.dispatchEvent(dragEvent('drop', { [COLUMN_DRAG_TYPE]: 'idea' }, { clientX: 846 }));
      await settle();

      expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);
      expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);
      expect(dropLines(root)).toEqual([]);
    }
  );

  it.each([
    ['names something off the row', (): DragPointer => ({ relatedTarget: document.body })],
    [
      'names nothing, the pointer above the row',
      (): DragPointer => ({ clientX: 846, clientY: -4 }),
    ],
  ] as const)(
    'forgets the line once the drag leaves the row, when the dragleave %s',
    async (_case, leaveTo) => {
      const root = await mountBoard();
      const row = layOutColumns(root);

      dragColumnOver(root, 'idea', 'doing', 'right');
      column(root, 'doing').dispatchEvent(dragEvent('dragleave', {}, leaveTo()));
      flushSync();

      expect(dropLines(root)).toEqual([]);

      // Back over the row, but over no column yet: nowhere to land.
      const over = dragEvent('dragover', { [COLUMN_DRAG_TYPE]: 'idea' }, { clientX: 846 });
      row.dispatchEvent(over);
      row.dispatchEvent(dragEvent('drop', { [COLUMN_DRAG_TYPE]: 'idea' }, { clientX: 846 }));
      await settle();

      expect(over.defaultPrevented).toBe(false);
      expect(commandsMock.moveTag).not.toHaveBeenCalled();
    }
  );

  it('still moves a card, even after a column drag that never ended', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    // The column drag's dragend never comes (its source left the page, say).
    dragColumnOver(root, 'idea', 'done', 'right');
    expect(dropLines(root)).toEqual(['done:after']);

    card(root, 'p2').dispatchEvent(dragEvent('dragstart', {}));
    const over = dragEvent(
      'dragover',
      { [PROJECT_DRAG_TYPE]: 'p2' },
      pointerOver(root, 'doing', 'right')
    );
    column(root, 'doing').dispatchEvent(over);
    flushSync();

    // A card's feedback, not a column's.
    expect(over.defaultPrevented).toBe(true);
    expect(column(root, 'doing').classList.contains('drop-target')).toBe(true);
    expect(dropLines(root)).toEqual([]);
    expect(column(root, 'idea').classList.contains('dragging')).toBe(false);

    column(root, 'doing').dispatchEvent(
      dragEvent('drop', { [PROJECT_DRAG_TYPE]: 'p2' }, pointerOver(root, 'doing', 'right'))
    );
    await settle();

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p2']);
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
  });

  it('moves a column, not a card, after a card drag that never ended', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    card(root, 'p2').dispatchEvent(dragEvent('dragstart', {}));
    column(root, 'doing').dispatchEvent(dragEvent('dragover', { [PROJECT_DRAG_TYPE]: 'p2' }));
    flushSync();
    expect(column(root, 'doing').classList.contains('drop-target')).toBe(true);

    dragColumnOver(root, 'idea', 'doing', 'right');

    expect(column(root, 'doing').classList.contains('drop-target')).toBe(false);
    expect(card(root, 'p2').classList.contains('dragging')).toBe(false);
    expect(dropLines(root)).toEqual(['doing:after']);

    await dropColumnOn(root, 'idea', 'doing', 'right');

    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
  });

  // A drop goes by what its drag carries. In each of these the board still
  // holds a drag of the other kind, never ended, and the drop comes from a
  // drag it did not see start (another window's), so nothing cleared it.

  it('takes a column’s drop as a column, with a card drag left over', async () => {
    const root = await mountBoard();
    layOutColumns(root);
    card(root, 'p2').dispatchEvent(dragEvent('dragstart', {}));

    const at = pointerOver(root, 'idea', 'left');
    column(root, 'idea').dispatchEvent(dragEvent('dragover', { [COLUMN_DRAG_TYPE]: 'done' }, at));
    column(root, 'idea').dispatchEvent(dragEvent('drop', { [COLUMN_DRAG_TYPE]: 'done' }, at));
    await settle();

    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
  });

  it('takes a card’s drop as a card, with a column drag left over', async () => {
    const root = await mountBoard();
    layOutColumns(root);
    dragColumnOver(root, 'idea', 'done', 'right');

    const at = pointerOver(root, 'doing', 'right');
    column(root, 'doing').dispatchEvent(dragEvent('dragover', { [PROJECT_DRAG_TYPE]: 'p1' }, at));
    column(root, 'doing').dispatchEvent(dragEvent('drop', { [PROJECT_DRAG_TYPE]: 'p1' }, at));
    await settle();

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p1', 'status', 'doing');
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
  });

  it('ignores text dragged over a column or the row, even text naming a column', async () => {
    const root = await mountBoard();
    const row = layOutColumns(root);

    const overColumn = dragEvent(
      'dragover',
      { 'text/plain': 'done' },
      pointerOver(root, 'idea', 'left')
    );
    column(root, 'idea').dispatchEvent(overColumn);
    column(root, 'idea').dispatchEvent(
      dragEvent('drop', { 'text/plain': 'done' }, pointerOver(root, 'idea', 'left'))
    );
    const overRow = dragEvent('dragover', { 'text/plain': 'done' }, { clientX: 846 });
    row.dispatchEvent(overRow);
    row.dispatchEvent(dragEvent('drop', { 'text/plain': 'done' }, { clientX: 846 }));
    await settle();

    expect(overColumn.defaultPrevented).toBe(false);
    expect(overRow.defaultPrevented).toBe(false);
    expect(dropLines(root)).toEqual([]);
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('does not take a card dropped in the gap as a column', async () => {
    const root = await mountBoard();
    const row = layOutColumns(root);

    card(root, 'p2').dispatchEvent(dragEvent('dragstart', {}));
    const over = dragEvent('dragover', { [PROJECT_DRAG_TYPE]: 'p2' }, { clientX: 846 });
    row.dispatchEvent(over);
    row.dispatchEvent(dragEvent('drop', { [PROJECT_DRAG_TYPE]: 'p2' }, { clientX: 846 }));
    await settle();

    expect(over.defaultPrevented).toBe(false);
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('drags a collapsed column by its strip', async () => {
    boardViewPrefs.columns = { status: { collapsed: ['done'], hidden: [] } };
    const root = await mountBoard();
    layOutColumns(root);
    const strip = columnHandle(root, 'done');
    expect(strip.classList.contains('collapsed-strip')).toBe(true);
    expect(strip.getAttribute('draggable')).toBe('true');

    await dragColumn(root, 'done', 'idea', 'left');

    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
    // Still collapsed where it landed.
    expect(columnHandle(root, 'done').classList.contains('collapsed-strip')).toBe(true);
  });

  it('never drags "No ‹group›", by its header or its strip', async () => {
    const root = await mountBoard();
    layOutColumns(root);
    expect(columnHandle(root, 'idea').getAttribute('draggable')).toBe('true');
    expect(columnHandle(root, NO_TAG_COLUMN_KEY).getAttribute('draggable')).toBe('false');

    // Even if an engine started the drag regardless.
    const start = dragEvent('dragstart', {});
    columnHandle(root, NO_TAG_COLUMN_KEY).dispatchEvent(start);
    flushSync();
    expect(dragTypes(start)).toEqual([]);
    expect(column(root, NO_TAG_COLUMN_KEY).classList.contains('dragging')).toBe(false);

    menuItem(await openColumnMenu(root, NO_TAG_COLUMN_KEY), 'Collapse').click();
    await settle();
    expect(columnHandle(root, NO_TAG_COLUMN_KEY).getAttribute('draggable')).toBe('false');
  });

  it('turns the header’s drag off while its name is being edited', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    menuItem(await openColumnMenu(root, 'doing'), 'Rename').click();
    await settle();
    const input = column(root, 'doing').querySelector<HTMLInputElement>('.column-name-input')!;
    expect(document.activeElement).toBe(input);
    expect(columnHandle(root, 'doing').getAttribute('draggable')).toBe('false');

    const start = dragEvent('dragstart', {});
    columnHandle(root, 'doing').dispatchEvent(start);
    flushSync();
    expect(dragTypes(start)).toEqual([]);
    expect(column(root, 'doing').classList.contains('dragging')).toBe(false);

    input.blur();
    await settle();
    expect(columnHandle(root, 'doing').getAttribute('draggable')).toBe('true');
  });

  it('leaves a drag that starts inside the header, a selection of its name, as text', async () => {
    const root = await mountBoard();

    const start = dragEvent('dragstart', { 'text/plain': 'Doing' });
    column(root, 'doing').querySelector('.column-name')!.dispatchEvent(start);
    flushSync();

    expect(dragTypes(start)).toEqual(['text/plain']);
    expect(column(root, 'doing').classList.contains('dragging')).toBe(false);
  });

  it('keeps a hidden column’s place among the columns a drag leaves alone', async () => {
    boardViewPrefs.columns = { status: { collapsed: [], hidden: ['doing'] } };
    const root = await mountBoard();
    layOutColumns(root);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'done']);

    // Done is already after Idea among those showing, with Doing hidden
    // between them: nothing to move.
    dragColumnOver(root, 'done', 'idea', 'right');
    expect(dropLines(root)).toEqual([]);
    await dropColumnOn(root, 'done', 'idea', 'right');
    expect(commandsMock.moveTag).not.toHaveBeenCalled();

    await dragColumn(root, 'idea', 'done', 'right');

    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
    expect(scopeIds(backend.state, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea']);
  });

  it('resolves a drop against the order the moves in flight show', async () => {
    const finishes = holdMoves();
    const root = await mountBoard();
    layOutColumns(root);

    await moveColumn(root, 'idea', 'Move right');
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);

    // Before Idea as shown is second. In the stored order Idea is still
    // first, and the same drop would put Done first.
    dragColumnOver(root, 'done', 'idea', 'left');
    expect(dropLines(root)).toEqual(['idea:before']);
    await dropColumnOn(root, 'done', 'idea', 'left');
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);

    finishes[0]();
    await settle();
    expect(commandsMock.moveTag.mock.calls).toEqual([
      ['idea', 'status', 1],
      ['done', 'status', 1],
    ]);

    finishes[1]();
    await settle();
    expect(scopeIds(backend.state, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
  });

  it('takes a column dragged in from another window, with no line until it drops', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    // No dragstart here: only the drop can say which column it is.
    const over = dragEvent(
      'dragover',
      { [COLUMN_DRAG_TYPE]: 'done' },
      pointerOver(root, 'idea', 'left')
    );
    column(root, 'idea').dispatchEvent(over);
    flushSync();
    expect(over.defaultPrevented).toBe(true);
    expect(dropLines(root)).toEqual([]);

    column(root, 'idea').dispatchEvent(
      dragEvent('drop', { [COLUMN_DRAG_TYPE]: 'done' }, pointerOver(root, 'idea', 'left'))
    );
    await settle();
    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);

    // A column of another group's board is none of this board's.
    column(root, 'idea').dispatchEvent(
      dragEvent('drop', { [COLUMN_DRAG_TYPE]: 'p0' }, pointerOver(root, 'idea', 'left'))
    );
    await settle();
    expect(commandsMock.moveTag).toHaveBeenCalledTimes(1);
  });

  describe('where cards do not drag', () => {
    let matchMedia: typeof window.matchMedia;

    beforeEach(() => {
      matchMedia = window.matchMedia;
    });

    afterEach(() => {
      window.matchMedia = matchMedia;
      viewport.hasCoarsePointer = false;
    });

    it('does not drag columns either, on a touch screen', async () => {
      // The query too: a menu mounting re-reads the viewport from it.
      window.matchMedia = ((query: string) => ({
        ...matchMedia(query),
        matches: query === '(pointer: coarse)',
      })) as typeof window.matchMedia;
      viewport.hasCoarsePointer = true;
      const root = await mountBoard();
      layOutColumns(root);
      expect(columnHandle(root, 'idea').getAttribute('draggable')).toBe('false');

      const over = dragColumnOver(root, 'idea', 'done', 'right');
      await dropColumnOn(root, 'idea', 'done', 'right');

      expect(over.defaultPrevented).toBe(false);
      expect(dropLines(root)).toEqual([]);
      expect(commandsMock.moveTag).not.toHaveBeenCalled();
    });
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
