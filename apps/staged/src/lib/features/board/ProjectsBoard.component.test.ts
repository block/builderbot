// Mounts the real board over the real tagsStore, tagActions and board
// preferences, with the backend commands and the preference store mocked: a
// drop or a "Move to" ends in `set_project_group_tag` and moves the card at
// once, and a collapsed column is saved to `projects-board-columns`. A card's
// labels are the real ones, so its chips show which tags its column leaves
// out. Moving a column from its menu ends in `move_tag`, through the real
// pending overlay, so the board shows it before the write lands, and so
// does dragging one by its header. Both drags are made of pointer events.
// A card's goes through boardCardDrag, and what a card is over is read off
// where the columns lie; jsdom lays nothing out, so the tests do
// (layOutBoard), with every column as tall as the board. A column's goes
// through the real pointerDrag action, at points taken from the columns'
// stubbed rects (layOutColumns), with the real chip following the pointer.
// The last two blocks put the real projectFiltersStore in front of it, as
// ProjectsList does, so the tag chips decide which cards it shows and the
// window's sort decides their order inside each column.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, TagState } from '../../types';
import { withTagMoved } from '../tags/tagCatalogPatches';
import { scopeIds, tagState, withGroupTag } from '../tags/testStubs/tagFixtures';
import type { DragPoint } from './boardCardDrag';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';
import { DRAG_CHIP_CLASS, DRAG_REACH, POINTER_DRAG_ATTRIBUTE } from '../../shared/pointerDrag';
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
const deleting = vi.hoisted(() => new Set<string>());

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
    isProjectDeleting: (projectId: string) => deleting.has(projectId),
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

import { toast } from 'svelte-sonner';
import { tagsStore } from '../../stores/tags.svelte';
import { projectFiltersStore } from '../projects/projectFilters.svelte';
import { setProjectSort } from '../projects/projectSortPrefs.svelte';
import FilteredBoardHarness from './testStubs/FilteredBoardHarness.svelte';
import { NO_TAG_COLUMN_KEY } from './boardPrefs';
import { boardViewPrefs } from './boardViewPrefs.svelte';
import ProjectsBoard from './ProjectsBoard.svelte';

let component: ReturnType<typeof mount> | null = null;
/** The mounted board's props, to change after the mount. */
let boardProps: { projects: Project[] };

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
  boardProps = props;
  component = mount(ProjectsBoard, { target, props });
  await settle();
  layOutBoard(target);
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

/** A point in client coordinates. */
interface Point {
  clientX: number;
  clientY: number;
}

/**
 * A drag event carrying a stub DataTransfer (jsdom has neither), with the
 * pointer at `at`.
 */
function dragEvent(type: string, data: Record<string, string>, at: Partial<Point> = {}): Event {
  const store = new Map(Object.entries(data));
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, {
    clientX: { value: at.clientX ?? 0 },
    clientY: { value: at.clientY ?? 0 },
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

/** How wide the columns' scroller is, from x = 0. */
let boardWidth = 1200;
/** The scroller runs from the top of the window down to here. */
const BOARD_BOTTOM = 600;
/** Under the board, the page's margin runs on down to the bottom of the window. */
const WINDOW_BOTTOM = 640;
/** How far the columns are scrolled to the left. */
let scrolledBy = 0;

/**
 * Something over the page at the pointer (a dialog), or null with the
 * pointer off the window; undefined leaves the board's layout to say what
 * the pointer is over.
 */
let cover: Element | null | undefined;
/** Where the pointer was last. */
let held: DragPoint = { x: 80, y: 10 };

function scrollerOf(root: ParentNode): HTMLElement {
  return root.querySelector<HTMLElement>('.board-columns')!;
}

/**
 * Lay the board out as a browser would: the columns' scroller is
 * `boardWidth` wide and BOARD_BOTTOM tall, and each column is as tall as it
 * (less the strip its scrollbar sits in), 272px wide and 12px apart from
 * x = 0. A collapsed column is 40px wide, as the + column is. Nothing
 * collapsed, that is "No ‹group›" 0–272, Idea 284–556, Doing 568–840, Done
 * 852–1124, then the + column 1136–1176.
 */
function layOutBoard(root: ParentNode) {
  const scroller = root.querySelector<HTMLElement>('.board-columns');
  if (!scroller) return;
  const box = (left: number, width: number, bottom: number) =>
    ({ left, right: left + width, top: 0, bottom, width, height: bottom }) as DOMRect;
  const widthOf = (zone: Element) => (zone.matches('.collapsed, [data-board-no-drop]') ? 40 : 272);
  scroller.getBoundingClientRect = () => box(0, boardWidth, BOARD_BOTTOM);
  for (const zone of scroller.children) {
    zone.getBoundingClientRect = () => {
      let left = -scrolledBy;
      for (const before of scroller.children) {
        if (before === zone) break;
        left += widthOf(before) + 12;
      }
      return box(left, widthOf(zone), BOARD_BOTTOM - 8);
    };
  }
}

/**
 * `document.elementFromPoint`, which jsdom hasn't got. With a card in hand
 * the board takes the pointer off everything in it but the columns'
 * scroller, so inside the scroller's box that is what is under the pointer;
 * anywhere else it is the bare page the board sits on.
 */
function elementAt(x: number, y: number): Element | null {
  if (cover !== undefined) return cover;
  const scroller = document.querySelector<HTMLElement>('.board-columns');
  if (!scroller) return document.body;
  const box = scroller.getBoundingClientRect();
  const inside = x >= box.left && x < box.right && y >= box.top && y < box.bottom;
  return inside ? scroller : scroller.closest('.projects-board')!.parentElement;
}

/** A pointer event as a mouse makes them (jsdom has no PointerEvent). */
function pointer(type: string, x: number, init: MouseEventInit = {}, pointerType = 'mouse'): Event {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: x,
    clientY: 10,
    button: 0,
    buttons: type === 'pointerup' ? 0 : 1,
    ...init,
  });
  Object.defineProperties(event, {
    pointerId: { value: 1 },
    pointerType: { value: pointerType },
    isPrimary: { value: true },
  });
  return event;
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

/** A point over the left or right half of a column (laid out by `layOutColumns`). */
function pointerOver(root: ParentNode, key: string, half: 'left' | 'right'): Point {
  const rect = column(root, key).getBoundingClientRect();
  return { clientX: half === 'left' ? rect.left + 100 : rect.right - 100, clientY: 20 };
}

/**
 * In the row, over no column: past Doing's right edge (840) and short of
 * Done's left (852), with all four columns showing. The line for a column
 * landing after Doing, or before Done, is drawn here.
 */
const IN_GAP: Point = { clientX: 846, clientY: 20 };

/** The gap after column `key`, between it and the next (laid out by `layOutColumns`). */
function gapAfter(root: ParentNode, key: string): Point {
  return { clientX: column(root, key).getBoundingClientRect().right + 6, clientY: 20 };
}

/** Level with that gap, and clearly above the row: one px past the drag's reach. */
const AWAY_FROM_ROW: Point = { clientX: 846, clientY: -1 - DRAG_REACH };

/**
 * A point as far across the row as the left or right half of a column, and
 * `clientY` down the page: above the row (which is 0 to 300 down) when
 * negative, below it past 300.
 */
function pointerLevelWith(
  root: ParentNode,
  key: string,
  half: 'left' | 'right',
  clientY: number
): Point {
  return { ...pointerOver(root, key, half), clientY };
}

/** The chip following the pointer, if a drag has one up. */
const dragChip = () => document.querySelector<HTMLElement>(`.${DRAG_CHIP_CLASS}`);

/** What a column drags by: its header's handle, or its strip when collapsed. */
function columnHandle(root: ParentNode, key: string): HTMLElement {
  return column(root, key).querySelector<HTMLElement>('.board-column-handle, .collapsed-strip')!;
}

/**
 * A mouse's pointer event at `at`, as the engines send it: the primary
 * button, down for all but the release.
 */
function pointerEvent(
  type: string,
  target: EventTarget,
  at: Point,
  init: PointerEventInit = {}
): PointerEvent {
  const event = new PointerEvent(type, {
    bubbles: true,
    cancelable: true,
    pointerId: 1,
    pointerType: 'mouse',
    isPrimary: true,
    button: type === 'pointermove' ? -1 : 0,
    buttons: type === 'pointerup' ? 0 : 1,
    ...at,
    ...init,
  });
  target.dispatchEvent(event);
  return event;
}

/** Where a press on column `key`'s handle lands: near the column's left edge. */
function pressPoint(root: ParentNode, key: string): Point {
  return { clientX: column(root, key).getBoundingClientRect().left + 20, clientY: 20 };
}

/** Press column `key`'s handle. No drag yet: the pointer has to move first. */
function pressColumn(root: ParentNode, key: string, init: PointerEventInit = {}) {
  pointerEvent('pointerdown', columnHandle(root, key), pressPoint(root, key), init);
}

/** Move the pressed pointer to `at`. The drag listens on the window, whatever is under it. */
function movePointer(at: Point) {
  pointerEvent('pointermove', document.body, at);
  flushSync();
}

/** Let the pointer go at `at`. */
async function releasePointer(at: Point) {
  pointerEvent('pointerup', document.body, at);
  await settle();
}

/** Pick column `key` up by its handle and hold it over a half of column `onto`. */
function dragColumnOver(root: ParentNode, key: string, onto: string, half: 'left' | 'right') {
  pressColumn(root, key);
  movePointer(pointerOver(root, onto, half));
}

/** Let go of the column being dragged over a half of column `onto`. */
async function dropColumnOn(root: ParentNode, onto: string, half: 'left' | 'right') {
  await releasePointer(pointerOver(root, onto, half));
}

/** Drag column `key` by its handle and drop it on a half of column `onto`. */
async function dragColumn(root: ParentNode, key: string, onto: string, half: 'left' | 'right') {
  dragColumnOver(root, key, onto, half);
  await dropColumnOn(root, onto, half);
}

/**
 * What the cursor says while a column is dragged, from the mark the drag
 * leaves on the root: `active` where a release would move the column,
 * `refused` where it would not, and null when nothing is being dragged.
 */
const dragMark = () => document.documentElement.getAttribute(POINTER_DRAG_ATTRIBUTE);

/** Each column showing a drop line, as `key:side`. */
function dropLines(root: ParentNode): string[] {
  return [...root.querySelectorAll<HTMLElement>('[data-board-column]')].flatMap((el) =>
    ['before', 'after']
      .filter((side) => el.classList.contains(`drop-${side}`))
      .map((side) => `${el.dataset.boardColumn}:${side}`)
  );
}

/** Halfway across a column or the + column, `y` down the window: near the top, unless said. */
function over(zone: Element, y = 10): DragPoint {
  const box = zone.getBoundingClientRect();
  return { x: (box.left + box.right) / 2, y };
}

/** Move the pointer, with whatever is in hand. */
async function moveTo(point: DragPoint) {
  held = point;
  window.dispatchEvent(pointer('pointermove', point.x, { clientY: point.y }));
  await settle();
}

/** Press a card and carry it to `to`, still held. */
async function carry(root: ParentNode, projectId: string, to: DragPoint) {
  layOutBoard(root);
  card(root, projectId).dispatchEvent(pointer('pointerdown', 10));
  await moveTo(to);
}

/** Let go of the card in hand, where it was last carried to. */
async function release() {
  window.dispatchEvent(pointer('pointerup', held.x, { clientY: held.y }));
  await settle();
}

async function dragTo(root: ParentNode, projectId: string, to: DragPoint) {
  await carry(root, projectId, to);
  await release();
}

/** The copy of the card that follows the pointer. */
function ghost(): HTMLElement | null {
  return document.querySelector<HTMLElement>('.board-drag-ghost');
}

function board(root: ParentNode): HTMLElement {
  return root.querySelector<HTMLElement>('.projects-board')!;
}

function addColumn(root: ParentNode): HTMLElement {
  return root.querySelector<HTMLElement>('[data-board-no-drop]')!;
}

/** The columns marked as the drop target: one at most. */
function lit(root: ParentNode): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>('.board-column.drop-target')];
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
  document.elementFromPoint = elementAt;
});

afterAll(() => {
  delete (document as Partial<Document>).elementFromPoint;
  delete (window as Partial<Window>).matchMedia;
  delete (globalThis as Partial<typeof globalThis>).ResizeObserver;
});

beforeEach(() => {
  vi.clearAllMocks();
  cover = undefined;
  held = { x: 80, y: 10 };
  boardWidth = 1200;
  scrolledBy = 0;
  deleting.clear();
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
  document.body.removeAttribute('style');
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

    await dragTo(root, 'p2', over(column(root, 'doing')));

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p2']);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual([]);
    expect(document.activeElement).toBe(card(root, 'p2'));

    // Far down the column, a long way under its one card, is the column still.
    await dragTo(root, 'p1', over(column(root, 'doing'), 580));

    expect(commandsMock.setProjectGroupTag).toHaveBeenLastCalledWith('p1', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p1', 'p2']);

    await dragTo(root, 'p1', over(column(root, NO_TAG_COLUMN_KEY), 580));

    expect(commandsMock.setProjectGroupTag).toHaveBeenLastCalledWith('p1', 'status', null);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p1']);
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledTimes(3);
  });

  it('keeps the move across a reload: the write is what the backend returns next', async () => {
    const root = await mountBoard();

    await dragTo(root, 'p2', over(column(root, 'done')));
    await tagsStore.load({ force: true });
    await settle();

    expect(cardsIn(root, 'done')).toEqual(['p1', 'p2']);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual([]);
  });

  it('shows the card in hand and marks the column under it', async () => {
    const root = await mountBoard();
    const doing = column(root, 'doing');
    expect(ghost()).toBeNull();

    await carry(root, 'p2', over(doing));

    // The card stays in its column, dimmed; its copy follows the pointer.
    expect(card(root, 'p2').classList.contains('dragging')).toBe(true);
    expect(ghost()?.textContent).toContain('Project p2');
    expect(ghost()?.querySelector('[data-board-card]')).toBeNull();
    expect(ghost()?.querySelector('.dragging')).toBeNull();
    expect(board(root).classList.contains('dragging')).toBe(true);
    expect(document.body.style.cursor).toBe('grabbing');
    expect(doing.classList.contains('drop-target')).toBe(true);
    expect(column(root, 'done').classList.contains('drop-target')).toBe(false);

    // The mark follows the pointer to the next column, and off the board,
    // past the + column.
    await moveTo(over(column(root, 'done')));
    expect(doing.classList.contains('drop-target')).toBe(false);
    expect(column(root, 'done').classList.contains('drop-target')).toBe(true);

    await moveTo({ x: 1190, y: 10 });
    expect(root.querySelector('.drop-target')).toBeNull();

    // Let go off the board: the card goes back, and nothing is written.
    await release();
    expect(ghost()).toBeNull();
    expect(card(root, 'p2').classList.contains('dragging')).toBe(false);
    expect(board(root).classList.contains('dragging')).toBe(false);
    expect(document.body.style.cursor).toBe('');
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('changes nothing when a card is dropped on its own column', async () => {
    const root = await mountBoard();
    const done = column(root, 'done');
    const elsewhere = card(root, 'p3');
    elsewhere.focus();

    // Its own column is not somewhere to move it to, however far down it.
    await carry(root, 'p1', over(done));
    expect(done.classList.contains('drop-target')).toBe(false);
    await moveTo(over(done, 580));
    expect(root.querySelector('.drop-target')).toBeNull();
    expect(board(root).classList.contains('drag-refused')).toBe(false);
    await release();

    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, 'done')).toEqual(['p1']);
    expect(document.activeElement).toBe(elsewhere);

    // The same for "No ‹group›", which would otherwise clear a tag it hasn't got.
    await dragTo(root, 'p2', over(column(root, NO_TAG_COLUMN_KEY)));
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('refuses a card on the + column, and says so', async () => {
    const root = await mountBoard();
    const add = addColumn(root);

    await carry(root, 'p2', over(add));

    expect(add.classList.contains('refusing')).toBe(true);
    expect(board(root).classList.contains('drag-refused')).toBe(true);
    expect(ghost()?.classList.contains('refused')).toBe(true);
    expect(document.body.style.cursor).toBe('not-allowed');
    expect(root.querySelector('.drop-target')).toBeNull();

    // Back over a column, it is welcome again.
    await moveTo(over(column(root, 'doing')));
    expect(add.classList.contains('refusing')).toBe(false);
    expect(document.body.style.cursor).toBe('grabbing');

    // The + refuses all the way down the board, not only on its button.
    await moveTo(over(add, 580));
    expect(add.classList.contains('refusing')).toBe(true);
    expect(document.body.style.cursor).toBe('not-allowed');
    await release();

    expect(add.classList.contains('refusing')).toBe(false);
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
    // Nor did the drop press the +.
    expect(add.querySelector('.add-form')).toBeNull();
  });

  it('takes a card anywhere down a column, however few cards it holds', async () => {
    const root = await mountBoard();
    const doing = column(root, 'doing');
    expect(cardsIn(root, 'doing')).toEqual([]);

    // Doing is empty: its foot is as much Doing as its header is.
    await carry(root, 'p2', over(doing, 580));
    expect(lit(root)).toEqual([doing]);

    // Done holds one card, at its top.
    await moveTo(over(column(root, 'done'), 590));
    expect(lit(root)).toEqual([column(root, 'done')]);

    await release();
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'done');
    expect(cardsIn(root, 'done')).toEqual(['p1', 'p2']);
  });

  it('lights up the box that takes the card: the column itself, as tall as the board', async () => {
    const root = await mountBoard();
    const scroller = scrollerOf(root);
    const doing = column(root, 'doing');
    const add = addColumn(root);

    // What the board stretches to its height is each child of the scroller
    // (in a browser; here layOutBoard does it): the columns and the + column.
    expect([...scroller.children]).toEqual([...root.querySelectorAll('[data-board-column]'), add]);

    // The pointer is in Doing's box, low down, and Doing's box is what is lit.
    await carry(root, 'p2', over(doing, 580));
    const box = doing.getBoundingClientRect();
    expect(held.x).toBeGreaterThan(box.left);
    expect(held.x).toBeLessThan(box.right);
    expect(held.y).toBeLessThan(box.bottom);
    expect(lit(root)).toEqual([doing]);
    expect(root.querySelectorAll('.drop-target')).toHaveLength(1);

    // The same for the + column and its refusal.
    await moveTo(over(add, 580));
    expect(held.y).toBeLessThan(add.getBoundingClientRect().bottom);
    expect([...root.querySelectorAll('.refusing')]).toEqual([add]);
    expect(lit(root)).toEqual([]);
    await release();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('gives the gap between two columns to the nearer one', async () => {
    const root = await mountBoard();
    const doing = column(root, 'doing');
    const done = column(root, 'done');

    // Doing ends at 840 and Done starts at 852.
    await carry(root, 'p2', { x: 838, y: 300 });
    expect(lit(root)).toEqual([doing]);
    // The mark never goes out as the card crosses from one to the other.
    for (let x = 839; x <= 846; x++) {
      await moveTo({ x, y: 300 });
      expect(lit(root)).toEqual([doing]);
    }
    for (let x = 847; x <= 853; x++) {
      await moveTo({ x, y: 300 });
      expect(lit(root)).toEqual([done]);
    }

    // Let go in the gap, nearer Done: it lands in Done.
    await moveTo({ x: 849, y: 580 });
    await release();
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'done');
    expect(cardsIn(root, 'done')).toEqual(['p1', 'p2']);

    // Nearer Doing: Doing.
    await dragTo(root, 'p2', { x: 843, y: 20 });
    expect(commandsMock.setProjectGroupTag).toHaveBeenLastCalledWith('p2', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p2']);
  });

  it('gives the gap beside the card’s own column, and beside the +, to them', async () => {
    const root = await mountBoard();
    const add = addColumn(root);

    // p1 is in Done, 852–1124; the + column starts at 1136. Done's half of
    // either gap is Done: nowhere to move p1 to.
    await carry(root, 'p1', { x: 848, y: 300 });
    expect(root.querySelector('.drop-target')).toBeNull();
    await moveTo({ x: 1128, y: 300 });
    expect(root.querySelector('.drop-target')).toBeNull();
    expect(add.classList.contains('refusing')).toBe(false);

    // The +'s half refuses, as the + does.
    await moveTo({ x: 1132, y: 300 });
    expect(add.classList.contains('refusing')).toBe(true);
    expect(document.body.style.cursor).toBe('not-allowed');

    await release();
    await dragTo(root, 'p1', { x: 848, y: 300 });
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, 'done')).toEqual(['p1']);
  });

  it('takes a card let go on the page under the board for the column above it', async () => {
    const root = await mountBoard();
    const doing = column(root, 'doing');

    // Below every column's foot, in the page's margin.
    await carry(root, 'p2', over(doing, BOARD_BOTTOM + 20));
    expect(lit(root)).toEqual([doing]);

    // In line with the + column it is refused, and past the board's side it
    // is over nothing.
    await moveTo(over(addColumn(root), BOARD_BOTTOM + 20));
    expect(addColumn(root).classList.contains('refusing')).toBe(true);
    await moveTo({ x: boardWidth + 10, y: BOARD_BOTTOM + 20 });
    expect(addColumn(root).classList.contains('refusing')).toBe(false);
    expect(root.querySelector('.drop-target')).toBeNull();

    // At the very bottom of the window.
    await moveTo(over(doing, WINDOW_BOTTOM - 1));
    expect(lit(root)).toEqual([doing]);
    await release();
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p2']);

    // Under its own column: nothing to change.
    await dragTo(root, 'p2', over(doing, WINDOW_BOTTOM - 1));
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledTimes(1);
  });

  it('changes nothing when a card is let go where something covers the board or the page', async () => {
    const root = await mountBoard();
    const dialog = document.createElement('div');
    document.body.appendChild(dialog);

    await carry(root, 'p1', over(column(root, 'doing'), 300));
    expect(lit(root)).toEqual([column(root, 'doing')]);

    // A dialog opens over the board: being in line with a column is not
    // being over it.
    cover = dialog;
    await moveTo(over(column(root, 'idea'), 300));
    expect(root.querySelector('.drop-target')).toBeNull();
    await moveTo(over(column(root, 'idea'), BOARD_BOTTOM + 20));
    expect(root.querySelector('.drop-target')).toBeNull();
    await release();

    // And with the pointer off the window.
    cover = null;
    await dragTo(root, 'p1', over(column(root, 'idea'), 300));

    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, 'done')).toEqual(['p1']);
  });

  it('leaves the part of a column that runs past the board’s side out of reach', async () => {
    const root = await mountBoard();
    const doing = column(root, 'doing');
    // 800px of the columns showing: Doing (568–840) runs 40px past the side.
    boardWidth = 800;

    await carry(root, 'p2', { x: 790, y: 300 });
    expect(lit(root)).toEqual([doing]);

    // Past the side, in line with the rest of Doing: on the page beside the
    // board, and under it.
    await moveTo({ x: 820, y: 300 });
    expect(root.querySelector('.drop-target')).toBeNull();
    await moveTo({ x: 820, y: BOARD_BOTTOM + 20 });
    expect(root.querySelector('.drop-target')).toBeNull();
    await release();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();

    await dragTo(root, 'p2', { x: 790, y: BOARD_BOTTOM + 20 });
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'doing');
  });

  it('reads the target again when the columns scroll under a held card', async () => {
    const root = await mountBoard();
    const doing = column(root, 'doing');

    await carry(root, 'p2', over(doing, 580));
    expect(doing.classList.contains('drop-target')).toBe(true);

    // The pointer is still; the columns moved a column's width to the left,
    // and Done is under it now.
    scrolledBy = 284;
    scrollerOf(root).dispatchEvent(new Event('scroll'));
    await settle();

    expect(doing.classList.contains('drop-target')).toBe(false);
    expect(column(root, 'done').classList.contains('drop-target')).toBe(true);
    await release();
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'done');
  });

  it('refuses a column whose tag was deleted while the card was in hand', async () => {
    const root = await mountBoard();
    const doing = column(root, 'doing');

    await carry(root, 'p2', over(doing, 580));
    expect(doing.classList.contains('drop-target')).toBe(true);

    // Another window deletes Doing; this one hasn't redrawn its column yet.
    tagsStore.patchLocal((state) => ({
      ...state,
      tags: state.tags.filter((tag) => tag.id !== 'doing'),
    }));
    await release();

    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
  });

  it('puts the card back, with a toast, when the write fails', async () => {
    const root = await mountBoard();
    let fail!: (error: Error) => void;
    commandsMock.setProjectGroupTag.mockImplementationOnce(
      () => new Promise<void>((_, reject) => (fail = reject))
    );

    await dragTo(root, 'p2', over(column(root, 'doing')));

    // Optimistic: the card is across before the backend answers.
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p2']);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual([]);
    expect(toast.error).not.toHaveBeenCalled();

    fail(new Error('database is locked'));
    await settle();

    expect(cardsIn(root, 'doing')).toEqual([]);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
    expect(toast.error).toHaveBeenCalledWith('Unable to update tags', {
      description: 'database is locked',
    });

    // And it can be dragged again.
    await dragTo(root, 'p2', over(column(root, 'doing')));
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledTimes(2);
    expect(cardsIn(root, 'doing')).toEqual(['p2']);
  });

  it('puts a moved card back when the write fails, not on "No ‹group›"', async () => {
    const root = await mountBoard();
    commandsMock.setProjectGroupTag.mockRejectedValueOnce(new Error('offline'));

    await dragTo(root, 'p1', over(column(root, NO_TAG_COLUMN_KEY)));

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p1', 'status', null);
    expect(cardsIn(root, 'done')).toEqual(['p1']);
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
    expect(toast.error).toHaveBeenCalledTimes(1);
  });

  it('keeps a press that doesn’t travel a click, and a drag from opening the card', async () => {
    const root = await mountBoard();
    const p2 = card(root, 'p2');

    // A click with a tremor: down, a pixel or two, up.
    p2.dispatchEvent(pointer('pointerdown', 10));
    window.dispatchEvent(pointer('pointermove', 12));
    window.dispatchEvent(pointer('pointerup', 12));
    p2.click();
    await settle();

    expect(openProject).toHaveBeenCalledWith('p2');
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(ghost()).toBeNull();
    openProject.mockClear();

    // A drag let go over the card itself: the click it ends in opens nothing.
    await carry(root, 'p2', { x: 80, y: 10 });
    window.dispatchEvent(pointer('pointerup', 80));
    p2.click();
    await settle();

    expect(openProject).not.toHaveBeenCalled();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();

    p2.click();
    expect(openProject).toHaveBeenCalledWith('p2');
  });

  it('opens a card from the keyboard after a drag whose release it never saw', async () => {
    const root = await mountBoard();
    const p3 = card(root, 'p3');

    await carry(root, 'p2', over(column(root, 'doing')));
    // Let go outside the window; the pointer comes back with no button down.
    window.dispatchEvent(pointer('pointermove', 90, { buttons: 0 }));
    await settle();

    expect(ghost()).toBeNull();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();

    // Enter on another card, with no press since: it is not the drag's click.
    p3.focus();
    p3.click();
    expect(openProject).toHaveBeenCalledWith('p3');
  });

  it('opens a card from the keyboard while a called-off drag’s button is still down', async () => {
    const root = await mountBoard();
    const p2 = card(root, 'p2');

    await carry(root, 'p2', { x: 80, y: 10 });
    window.dispatchEvent(new Event('blur'));
    await settle();
    expect(ghost()).toBeNull();

    card(root, 'p3').click();
    expect(openProject).toHaveBeenCalledWith('p3');
    openProject.mockClear();

    // The release over the card, when the page does see it, opens nothing.
    window.dispatchEvent(pointer('pointerup', 80));
    p2.click();
    await settle();
    expect(openProject).not.toHaveBeenCalled();
  });

  it('puts the ghost away and gives the page back when unmounted with a card in hand', async () => {
    const portalRoot = document.createElement('div');
    portalRoot.id = 'portal-root';
    document.body.appendChild(portalRoot);
    document.body.style.cursor = 'progress';
    document.body.style.setProperty('user-select', 'text');
    const root = await mountBoard();

    await carry(root, 'p2', over(column(root, 'doing')));
    expect(portalRoot.querySelector('.board-drag-ghost')).not.toBeNull();
    expect(document.body.style.cursor).toBe('grabbing');
    expect(document.body.style.getPropertyValue('user-select')).toBe('none');
    expect(document.body.style.getPropertyValue('-webkit-user-select')).toBe('none');

    void unmount(component!);
    component = null;
    await settle();

    expect(portalRoot.childElementCount).toBe(0);
    expect(ghost()).toBeNull();
    // As they were before the drag, not merely cleared.
    expect(document.body.style.cursor).toBe('progress');
    expect(document.body.style.getPropertyValue('user-select')).toBe('text');
    expect(document.body.style.getPropertyValue('-webkit-user-select')).toBe('');

    // The drag is over: the late release writes nothing, and its click is
    // nobody's to swallow twice.
    await release();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('calls a drag off on Escape', async () => {
    const root = await mountBoard();

    await carry(root, 'p2', over(column(root, 'doing'), 580));
    expect(lit(root)).toEqual([column(root, 'doing')]);
    document.body.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    );
    await settle();

    expect(ghost()).toBeNull();
    expect(root.querySelector('.drop-target')).toBeNull();

    await release();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
  });

  it('leaves the context menu’s press, and a touch, alone', async () => {
    const root = await mountBoard();
    const doing = column(root, 'doing');

    // Each ends over Doing (568–840), where a drag would have had a target.
    card(root, 'p2').dispatchEvent(pointer('pointerdown', 10, { button: 2, buttons: 2 }));
    window.dispatchEvent(pointer('pointermove', 700, { buttons: 2 }));
    window.dispatchEvent(pointer('pointerup', 700));
    await settle();

    card(root, 'p2').dispatchEvent(pointer('pointerdown', 10, {}, 'touch'));
    held = { x: 700, y: 10 };
    window.dispatchEvent(pointer('pointermove', 700));
    await settle();

    expect(ghost()).toBeNull();
    expect(doing.classList.contains('drop-target')).toBe(false);
    await release();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
  });

  it('leaves the card of a project being deleted where it is', async () => {
    const root = await mountBoard();
    deleting.add('p2');

    await dragTo(root, 'p2', over(column(root, 'doing')));

    expect(ghost()).toBeNull();
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect(cardsIn(root, NO_TAG_COLUMN_KEY)).toEqual(['p2']);
  });

  it('lets go of a card whose project leaves the board mid-drag', async () => {
    const root = await mountBoard();

    await carry(root, 'p2', over(column(root, 'doing')));
    expect(ghost()).not.toBeNull();
    boardProps.projects = projects.filter((project) => project.id !== 'p2');
    await settle();

    expect(ghost()).toBeNull();
    expect(root.querySelector('.drop-target')).toBeNull();
    expect(document.body.style.cursor).toBe('');
    await release();
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

    // Low on the strip, which is as tall as the board: 40px wide, from 852.
    expect(over(done).x).toBe(872);
    await carry(root, 'p3', over(done, 580));

    expect(done.classList.contains('drop-target')).toBe(true);
    expect(strip.classList.contains('drop-target')).toBe(true);

    await release();

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

    // A press alone is no drag: the pointer has to move first.
    pressColumn(root, 'idea');
    flushSync();
    expect(dragMark()).toBeNull();
    expect(column(root, 'idea').classList.contains('dragging')).toBe(false);

    movePointer(pointerOver(root, 'done', 'right'));
    expect(dragMark()).toBe('active');
    expect(dropLines(root)).toEqual(['done:after']);
    expect(column(root, 'idea').classList.contains('dragging')).toBe(true);

    await dropColumnOn(root, 'done', 'right');

    // The write is still open, and the board already shows the new order.
    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
    expect(scopeIds(backend.state, 'status')).toEqual(['idea', 'doing', 'done']);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
    expect(cardsIn(root, 'idea')).toEqual(['p3']);
    expect(dragMark()).toBeNull();
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
    await dropColumnOn(root, 'idea', 'left');

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
      await dropColumnOn(root, NO_TAG_COLUMN_KEY, half);

      expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
      expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
    }
  );

  it.each([
    ['on itself', 'doing', 'doing', 'left', 'idea'],
    ['on the near side of the column after it', 'idea', 'doing', 'left', 'idea'],
    ['on the near side of the column before it', 'doing', 'idea', 'right', 'idea'],
    ['on "No ‹group›" when already first', 'idea', NO_TAG_COLUMN_KEY, 'right', NO_TAG_COLUMN_KEY],
  ] as const)(
    'shows no line, refuses the drop and writes nothing for a drop %s',
    async (_case, key, onto, half, gapFollows) => {
      const root = await mountBoard();
      layOutColumns(root);

      dragColumnOver(root, key, onto, half);

      // The column is being dragged, and the cursor says a release here
      // would not move it.
      expect(column(root, key).classList.contains('dragging')).toBe(true);
      expect(dragMark()).toBe('refused');
      expect(dropLines(root)).toEqual([]);

      // The gap that place's line would be drawn in is the same place.
      movePointer(gapAfter(root, gapFollows));
      expect(dragMark()).toBe('refused');
      expect(dropLines(root)).toEqual([]);

      await dropColumnOn(root, onto, half);

      expect(commandsMock.moveTag).not.toHaveBeenCalled();
      expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
      expect(dragMark()).toBeNull();
      expect(column(root, key).classList.contains('dragging')).toBe(false);
    }
  );

  it('says a drop moves again once the column is held over a half that moves it', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    // Idea over its own left half, then over the far half of Doing.
    dragColumnOver(root, 'idea', 'idea', 'left');
    expect(dragMark()).toBe('refused');
    movePointer(pointerOver(root, 'doing', 'right'));

    expect(dragMark()).toBe('active');
    expect(dropLines(root)).toEqual(['doing:after']);

    // And in the gap, where that line is drawn.
    movePointer(IN_GAP);
    expect(dragMark()).toBe('active');

    // Back over itself, it is refused again.
    movePointer(pointerOver(root, 'idea', 'left'));
    expect(dragMark()).toBe('refused');
    expect(dropLines(root)).toEqual([]);
  });

  it('takes a release in the gap, where the line is drawn', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'doing', 'right');
    expect(dropLines(root)).toEqual(['doing:after']);

    // Off Doing and into the gap, where the line is drawn.
    movePointer(IN_GAP);
    expect(dragMark()).toBe('active');
    expect(dropLines(root)).toEqual(['doing:after']);

    await releasePointer(IN_GAP);

    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);
    expect(dropLines(root)).toEqual([]);
  });

  it('forgets the line once the pointer is clearly away from the row', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'doing', 'right');
    movePointer(AWAY_FROM_ROW);

    expect(dropLines(root)).toEqual([]);
    expect(dragMark()).toBe('refused');
    // Still being dragged: only a release or a cancel ends that.
    expect(column(root, 'idea').classList.contains('dragging')).toBe(true);

    // Back within reach, there is somewhere to land at once, with no
    // column crossed on the way: the gap after Done is a place of its own.
    movePointer(gapAfter(root, 'done'));
    expect(dropLines(root)).toEqual(['done:after']);
    expect(dragMark()).toBe('active');
    await releasePointer(gapAfter(root, 'done'));

    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
  });

  it('lands a column in a gap it reaches without crossing a column', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    // Straight from the press into the gap after Idea: one move.
    pressColumn(root, 'done');
    movePointer(gapAfter(root, 'idea'));
    expect(dropLines(root)).toEqual(['idea:after']);
    expect(dragMark()).toBe('active');

    await releasePointer(gapAfter(root, 'idea'));
    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 1]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'done', 'doing']);
  });

  it('does not carry a line from one gap to another', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    // Done held after "No ‹group›", then moved to the gap after Idea in
    // one move, over no column on the way.
    dragColumnOver(root, 'done', NO_TAG_COLUMN_KEY, 'right');
    expect(dropLines(root)).toEqual([`${NO_TAG_COLUMN_KEY}:after`]);
    movePointer(gapAfter(root, 'idea'));

    expect(dropLines(root)).toEqual(['idea:after']);
  });

  it('takes the line up again once the pointer is back level with a column', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'doing', 'right');
    movePointer(AWAY_FROM_ROW);
    movePointer(pointerOver(root, 'done', 'right'));

    expect(dragMark()).toBe('active');
    expect(dropLines(root)).toEqual(['done:after']);
  });

  // A header is at the row's top edge, so a pointer dragging a column
  // sideways drifts above the row. Only how far across it is counts.

  it.each([
    ['just above', -20],
    ['as far above as the drag reaches', -DRAG_REACH],
    ['just below', 320],
    ['as far below as the drag reaches', 299 + DRAG_REACH],
  ])('keeps the line for a pointer that drifts %s the row, and lands there', async (_drift, y) => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'doing', 'right');
    movePointer(pointerLevelWith(root, 'done', 'right', y));

    expect(dropLines(root)).toEqual(['done:after']);
    expect(dragMark()).toBe('active');

    // The other half of the column is still the other side.
    movePointer(pointerLevelWith(root, 'done', 'left', y));
    expect(dropLines(root)).toEqual(['done:before']);

    await releasePointer(pointerLevelWith(root, 'done', 'right', y));

    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
  });

  it('names a column from above the row without the pointer having been in it', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    // Straight from the press to a point above Done: one move.
    pressColumn(root, 'idea');
    movePointer(pointerLevelWith(root, 'done', 'right', -30));

    expect(dropLines(root)).toEqual(['done:after']);
    expect(dragMark()).toBe('active');
  });

  it('still refuses a drop that moves nothing, from above the row', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    pressColumn(root, 'idea');
    movePointer(pointerLevelWith(root, 'idea', 'right', -30));

    expect(dropLines(root)).toEqual([]);
    expect(dragMark()).toBe('refused');
    await releasePointer(pointerLevelWith(root, 'idea', 'right', -30));
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
  });

  it('lands a column last when it is held past the last column', async () => {
    const root = await mountBoard();
    const row = layOutColumns(root);
    // Done, the last column, ends at 1124. The + column and the row's end
    // (1420) are past it.
    const overAddColumn = { clientX: 1200, clientY: 20 };
    const pastTheRow = { clientX: row.getBoundingClientRect().right + DRAG_REACH - 1, clientY: 20 };

    // Straight from the press: no column was crossed on the way.
    pressColumn(root, 'idea');
    movePointer(overAddColumn);
    expect(dropLines(root)).toEqual(['done:after']);
    expect(dragMark()).toBe('active');

    movePointer(pastTheRow);
    expect(dropLines(root)).toEqual(['done:after']);
    expect(dragMark()).toBe('active');

    await releasePointer(pastTheRow);
    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
  });

  it('lands a column first when it is held past the row’s left end', async () => {
    const root = await mountBoard();
    layOutColumns(root);
    const pastTheRow = { clientX: -DRAG_REACH, clientY: 20 };

    pressColumn(root, 'done');
    movePointer(pastTheRow);
    // "No ‹group›" is at that end, and a column only ever lands after it.
    expect(dropLines(root)).toEqual([`${NO_TAG_COLUMN_KEY}:after`]);
    expect(dragMark()).toBe('active');

    await releasePointer(pastTheRow);
    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
  });

  it('lands nowhere once the pointer is past the row’s end by more than the reach', async () => {
    const root = await mountBoard();
    const row = layOutColumns(root);
    const right = row.getBoundingClientRect().right;

    pressColumn(root, 'idea');
    movePointer({ clientX: right + DRAG_REACH, clientY: 20 });
    expect(dropLines(root)).toEqual([]);
    expect(dragMark()).toBe('refused');

    movePointer({ clientX: -1 - DRAG_REACH, clientY: 20 });
    expect(dropLines(root)).toEqual([]);
    expect(dragMark()).toBe('refused');

    await releasePointer({ clientX: -1 - DRAG_REACH, clientY: 20 });
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
  });

  it('refuses the last column held past the end, where it already is', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    pressColumn(root, 'done');
    movePointer({ clientX: 1200, clientY: 20 });

    expect(dropLines(root)).toEqual([]);
    expect(dragMark()).toBe('refused');
  });

  // The chip that follows the pointer: the column's icon and name.

  it('shows a chip with the column’s icon and name while it is dragged', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    pressColumn(root, 'doing');
    flushSync();
    expect(dragChip()).toBeNull();

    movePointer(pointerOver(root, 'done', 'right'));
    const chip = dragChip()!;
    expect(chip.textContent).toBe(column(root, 'doing').querySelector('.column-name')!.textContent);
    // A copy of the header's icon, which stays in the header.
    const icon = columnHandle(root, 'doing').querySelector('svg')!;
    expect(chip.querySelector('svg')).not.toBeNull();
    expect(chip.querySelector('svg')).not.toBe(icon);
    expect(chip.querySelector('svg')!.outerHTML).toBe(icon.outerHTML);
    // Out of the board, so it takes no room in the row and no events.
    expect(root.contains(chip)).toBe(false);
    expect(chip.style.position).toBe('fixed');
    expect(chip.style.pointerEvents).toBe('none');

    // It follows the pointer, and there is only ever one.
    const placed = chip.style.transform;
    movePointer(pointerOver(root, 'idea', 'left'));
    expect(dragChip()).toBe(chip);
    expect(chip.style.transform).not.toBe(placed);
    expect(document.querySelectorAll(`.${DRAG_CHIP_CLASS}`)).toHaveLength(1);

    await dropColumnOn(root, 'idea', 'left');
    expect(dragChip()).toBeNull();
  });

  it('keeps the chip up where a drop is refused, and takes it down on that release', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'idea', 'left');
    expect(dragMark()).toBe('refused');
    expect(dragChip()).not.toBeNull();
    movePointer(AWAY_FROM_ROW);
    expect(dragChip()).not.toBeNull();

    await releasePointer(AWAY_FROM_ROW);
    expect(dragChip()).toBeNull();
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
  });

  it('takes the chip down when Escape ends the drag', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'done', 'right');
    expect(dragChip()).not.toBeNull();
    document.body.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    );
    flushSync();

    expect(dragChip()).toBeNull();
    // The button is still down: a move does not bring it back.
    movePointer(pointerOver(root, 'done', 'right'));
    expect(dragChip()).toBeNull();
  });

  it('takes the chip down when the window loses focus, or the board goes', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'done', 'right');
    expect(dragChip()).not.toBeNull();
    window.dispatchEvent(new FocusEvent('blur'));
    expect(dragChip()).toBeNull();
    await releasePointer(pointerOver(root, 'done', 'right'));

    dragColumnOver(root, 'idea', 'done', 'right');
    expect(dragChip()).not.toBeNull();
    unmount(component!);
    component = null;
    expect(dragChip()).toBeNull();
    expect(dragMark()).toBeNull();
  });

  // A release is resolved where it happens, not where the pointer last
  // moved: the two need not be the same point.

  it('writes nothing for a release clearly away from the row, with a line showing until then', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'done', 'right');
    expect(dropLines(root)).toEqual(['done:after']);
    await releasePointer(AWAY_FROM_ROW);

    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
    expect(dropLines(root)).toEqual([]);
    expect(column(root, 'idea').classList.contains('dragging')).toBe(false);
  });

  it('lands the column where it is released, over another column than the last move', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'doing', 'right');
    await dropColumnOn(root, 'done', 'right');

    expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);
  });

  it('writes nothing when Escape ends the drag', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'idea', 'done', 'right');
    expect(dropLines(root)).toEqual(['done:after']);

    const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    document.body.dispatchEvent(escape);
    flushSync();

    // The drag took the key, and is over with the button still down.
    expect(escape.defaultPrevented).toBe(true);
    expect(dragMark()).toBeNull();
    expect(dropLines(root)).toEqual([]);
    expect(column(root, 'idea').classList.contains('dragging')).toBe(false);

    // Neither a move nor the release after it brings it back.
    movePointer(pointerOver(root, 'done', 'right'));
    expect(dropLines(root)).toEqual([]);
    await dropColumnOn(root, 'done', 'right');

    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
  });

  it('marks the first column in the row, whose drop line sits on its own edge', async () => {
    const firstInRow = (root: ParentNode) =>
      [...root.querySelectorAll<HTMLElement>('.board-column.first-in-row')].map(
        (el) => el.dataset.boardColumn
      );
    const root = await mountBoard();
    layOutColumns(root);

    // "No ‹group›" is first while it shows, and never takes a line before it.
    expect(firstInRow(root)).toEqual([NO_TAG_COLUMN_KEY]);

    // Hidden, it leaves a tag column first, at the row's edge: the line
    // before that column has no gap to sit in.
    menuItem(await openColumnMenu(root, NO_TAG_COLUMN_KEY), 'Hide column').click();
    await settle();
    expect(columnKeys(root)).toEqual(['idea', 'doing', 'done']);
    expect(firstInRow(root)).toEqual(['idea']);

    dragColumnOver(root, 'done', 'idea', 'left');
    expect(dropLines(root)).toEqual(['idea:before']);
    expect(firstInRow(root)).toEqual(['idea']);
    await releasePointer(AWAY_FROM_ROW);

    // A line before any other column keeps to the gap.
    dragColumnOver(root, 'idea', 'done', 'left');
    expect(dropLines(root)).toEqual(['done:before']);
    expect(firstInRow(root)).toEqual(['idea']);
    await releasePointer(AWAY_FROM_ROW);
    expect(commandsMock.moveTag).not.toHaveBeenCalled();

    // The mark follows the order: Done dropped first becomes the first.
    await dragColumn(root, 'done', 'idea', 'left');
    expect(columnKeys(root)).toEqual(['done', 'idea', 'doing']);
    expect(firstInRow(root)).toEqual(['done']);
  });

  it('moves no column for a card dragged across the row', async () => {
    const root = await mountBoard();

    // A card in hand is no column drag: no drop line, and no mark on the root.
    await carry(root, 'p2', over(column(root, 'doing')));
    expect(dropLines(root)).toEqual([]);
    expect(dragMark()).toBeNull();
    await release();

    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p2', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p2']);
  });

  it('drags a collapsed column by its strip, and does not expand it for the release', async () => {
    boardViewPrefs.columns = { status: { collapsed: ['done'], hidden: [] } };
    const root = await mountBoard();
    layOutColumns(root);
    const strip = columnHandle(root, 'done');
    expect(strip.classList.contains('collapsed-strip')).toBe(true);

    dragColumnOver(root, 'done', 'idea', 'left');
    expect(dropLines(root)).toEqual(['idea:before']);
    // The release, and the click an engine sends straight after it.
    pointerEvent('pointerup', document.body, pointerOver(root, 'idea', 'left'));
    strip.click();
    await settle();

    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
    // Still collapsed where it landed: the click was the drag's, not a
    // request to expand.
    expect(columnHandle(root, 'done').classList.contains('collapsed-strip')).toBe(true);
    expect(setStoreValue).not.toHaveBeenCalled();

    // The next click is its own.
    columnHandle(root, 'done').click();
    await settle();
    expect(columnHandle(root, 'done').classList.contains('collapsed-strip')).toBe(false);
  });

  it.each([
    ['never moves', 0],
    ['moves less than a drag takes', 2],
  ])('still expands a collapsed column on a press that %s', async (_case, travel) => {
    boardViewPrefs.columns = { status: { collapsed: ['done'], hidden: [] } };
    const root = await mountBoard();
    layOutColumns(root);
    const strip = columnHandle(root, 'done');
    const pressed = pressPoint(root, 'done');
    const released = { ...pressed, clientX: pressed.clientX + travel };

    pressColumn(root, 'done');
    if (travel > 0) movePointer(released);
    expect(dragMark()).toBeNull();
    expect(column(root, 'done').classList.contains('dragging')).toBe(false);
    pointerEvent('pointerup', document.body, released);
    strip.click();
    await settle();

    expect(columnHandle(root, 'done').classList.contains('collapsed-strip')).toBe(false);
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
  });

  it('does not drag by the ⋯ menu, which is not part of the handle', async () => {
    const root = await mountBoard();
    layOutColumns(root);
    const trigger = columnMenuTrigger(root, 'idea');
    expect(columnHandle(root, 'idea').contains(trigger)).toBe(false);

    pointerEvent('pointerdown', trigger, pressPoint(root, 'idea'));
    movePointer(pointerOver(root, 'done', 'right'));

    expect(dragMark()).toBeNull();
    expect(column(root, 'idea').classList.contains('dragging')).toBe(false);
    await dropColumnOn(root, 'done', 'right');
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
  });

  it('never drags "No ‹group›", by its header or its strip', async () => {
    const root = await mountBoard();
    layOutColumns(root);
    expect(columnHandle(root, 'idea').classList.contains('draggable')).toBe(true);
    expect(columnHandle(root, NO_TAG_COLUMN_KEY).classList.contains('draggable')).toBe(false);

    dragColumnOver(root, NO_TAG_COLUMN_KEY, 'done', 'right');
    expect(dragMark()).toBeNull();
    expect(dropLines(root)).toEqual([]);
    expect(column(root, NO_TAG_COLUMN_KEY).classList.contains('dragging')).toBe(false);
    await dropColumnOn(root, 'done', 'right');

    menuItem(await openColumnMenu(root, NO_TAG_COLUMN_KEY), 'Collapse').click();
    await settle();
    const strip = columnHandle(root, NO_TAG_COLUMN_KEY);
    expect(strip.classList.contains('collapsed-strip')).toBe(true);
    expect(strip.classList.contains('draggable')).toBe(false);

    dragColumnOver(root, NO_TAG_COLUMN_KEY, 'done', 'right');
    expect(dragMark()).toBeNull();
    expect(column(root, NO_TAG_COLUMN_KEY).classList.contains('dragging')).toBe(false);
    await dropColumnOn(root, 'done', 'right');

    expect(commandsMock.moveTag).not.toHaveBeenCalled();
  });

  it('turns the header’s drag off while its name is being edited', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    menuItem(await openColumnMenu(root, 'doing'), 'Rename').click();
    await settle();
    const input = column(root, 'doing').querySelector<HTMLInputElement>('.column-name-input')!;
    expect(document.activeElement).toBe(input);
    expect(columnHandle(root, 'doing').contains(input)).toBe(true);
    expect(columnHandle(root, 'doing').classList.contains('draggable')).toBe(false);

    dragColumnOver(root, 'doing', 'idea', 'left');
    expect(dragMark()).toBeNull();
    expect(dropLines(root)).toEqual([]);
    expect(column(root, 'doing').classList.contains('dragging')).toBe(false);
    await dropColumnOn(root, 'idea', 'left');
    expect(commandsMock.moveTag).not.toHaveBeenCalled();

    // The pointer can drag a selection of the field's text about.
    const start = dragEvent('dragstart', { 'text/plain': 'Doing' });
    input.dispatchEvent(start);
    expect(start.defaultPrevented).toBe(false);

    input.blur();
    await settle();
    expect(columnHandle(root, 'doing').classList.contains('draggable')).toBe(true);
    await dragColumn(root, 'doing', 'idea', 'left');
    expect(commandsMock.moveTag.mock.calls).toEqual([['doing', 'status', 0]]);
  });

  // After a card's drag, which is a native one, WebKit sees no release and
  // takes the mouse button to be down. It sends the next press as a move
  // that names the primary button, with a mousedown and no pointerdown. The
  // first column drag after a card drag did nothing.

  it('drags a column whose press the engine sent as a move, after a native drag', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    const handle = columnHandle(root, 'done');
    pointerEvent('pointermove', handle, pressPoint(root, 'done'), { button: 0 });
    handle.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
    movePointer(pointerOver(root, 'idea', 'left'));
    expect(dragMark()).toBe('active');
    expect(dropLines(root)).toEqual(['idea:before']);

    // The click that the release sends comes in the same task, and goes to
    // what the press and the release share: the row, or further out. It
    // used to reach it. It is swallowed.
    pointerEvent('pointerup', document.body, pointerOver(root, 'idea', 'left'));
    const click = new MouseEvent('click', { bubbles: true, cancelable: true });
    root.querySelector('.board-columns')!.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    await settle();

    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
  });

  // WebKit sends that same move, with no mousedown, as the first move after
  // another button is clicked while the primary one is held. It is not a
  // press: a press made elsewhere, carried over a column's handle and
  // right-clicked there, dragged the column.

  it('does not drag a column from a move that only looks like a press', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    pointerEvent('pointermove', columnHandle(root, 'done'), pressPoint(root, 'done'), {
      button: 0,
    });
    movePointer(pointerOver(root, 'idea', 'left'));
    expect(dragMark()).toBeNull();
    expect(dropLines(root)).toEqual([]);
    await releasePointer(pointerOver(root, 'idea', 'left'));

    expect(commandsMock.moveTag).not.toHaveBeenCalled();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
  });

  // A right click in the middle of a column drag is not the end of it.
  // WebKit sends the click one of two ways. Where the context menu is
  // prevented, and for a middle click, it is two moves that name the
  // button, and the next move then looks like a press of the primary one.
  // That move cancelled the drag. With the context menu left alone, the
  // right button's release is a pointerup with the primary button still
  // down. That dropped the column where it stood. A drag now prevents the
  // menu, and takes only the primary button's release for its drop.

  it.each([
    ['as moves, the next one shaped like a press', 'moves'],
    ['with its release a pointerup', 'release'],
  ] as const)('keeps a column drag running through a right click sent %s', async (_name, sent) => {
    const root = await mountBoard();
    layOutColumns(root);

    dragColumnOver(root, 'done', 'doing', 'left');
    expect(dropLines(root)).toEqual(['doing:before']);
    const at = pointerOver(root, 'doing', 'left');
    pointerEvent('pointermove', document.body, at, { button: 2, buttons: 3 });
    document.body.dispatchEvent(
      new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 2, buttons: 3 })
    );
    pointerEvent(sent === 'moves' ? 'pointermove' : 'pointerup', document.body, at, {
      button: 2,
      buttons: 1,
    });
    flushSync();
    expect(dragMark()).toBe('active');
    expect(dragChip()).not.toBeNull();
    expect(dropLines(root)).toEqual(['doing:before']);
    expect(commandsMock.moveTag).not.toHaveBeenCalled();

    // The first move after it: the one that names the primary button.
    pointerEvent('pointermove', document.body, pointerOver(root, 'idea', 'left'), {
      button: sent === 'moves' ? 0 : -1,
    });
    flushSync();
    expect(dragMark()).toBe('active');
    expect(dropLines(root)).toEqual(['idea:before']);
    await dropColumnOn(root, 'idea', 'left');

    expect(dragChip()).toBeNull();
    expect(commandsMock.moveTag.mock.calls).toEqual([['done', 'status', 0]]);
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
  });

  // The press on a handle has no default action, so the engine starts no
  // selection to extend as the pointer moves. WebKit put that selection,
  // and focus with it, into a field the pointer passed: the Search field
  // above the board.

  it('cancels the default of a press on a column’s handle, and takes focus off a field', async () => {
    const root = await mountBoard();
    layOutColumns(root);
    const search = document.createElement('input');
    document.body.append(search);
    search.focus();
    const mousedown = (target: Element) => {
      const event = new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0 });
      target.dispatchEvent(event);
      return event;
    };

    try {
      pressColumn(root, 'done');
      const name = column(root, 'done').querySelector('.column-name')!;
      expect(mousedown(name).defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(document.body);

      // Focus is where the press left it through the drag and after the drop.
      movePointer(pointerOver(root, 'idea', 'left'));
      await dropColumnOn(root, 'idea', 'left');
      expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'done', 'idea', 'doing']);
      expect(document.activeElement).toBe(document.body);

      // "No ‹group›" does not drag, so a press on its header is an
      // ordinary one. So is one on a column's menu trigger, beside the
      // handle, and on a card.
      search.focus();
      pressColumn(root, NO_TAG_COLUMN_KEY);
      expect(mousedown(columnHandle(root, NO_TAG_COLUMN_KEY)).defaultPrevented).toBe(false);
      const trigger = column(root, 'idea').querySelector('.board-column-menu-trigger')!;
      expect(mousedown(trigger).defaultPrevented).toBe(false);
      expect(mousedown(card(root, 'p1')).defaultPrevented).toBe(false);
      expect(document.activeElement).toBe(search);
    } finally {
      search.remove();
    }
  });

  it('leaves a press in a column’s name field alone while the name is edited', async () => {
    const root = await mountBoard();
    layOutColumns(root);
    menuItem(await openColumnMenu(root, 'doing'), 'Rename').click();
    await settle();
    const input = column(root, 'doing').querySelector<HTMLInputElement>('.column-name-input')!;

    // The field is inside the handle, which is off: the press places the
    // caret and can select the name's text.
    pointerEvent('pointerdown', input, pressPoint(root, 'doing'));
    const event = new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0 });
    input.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(input);
  });

  it('refuses a native drag that starts in the handle, which would take the pointer', async () => {
    const root = await mountBoard();

    const start = dragEvent('dragstart', { 'text/plain': 'Doing' });
    column(root, 'doing').querySelector('.column-name')!.dispatchEvent(start);
    flushSync();

    expect(start.defaultPrevented).toBe(true);
    expect(column(root, 'doing').classList.contains('dragging')).toBe(false);

    // A card refuses one too: its drag is made of pointer events as well.
    const cardStart = dragEvent('dragstart', {});
    card(root, 'p1').dispatchEvent(cardStart);
    expect(cardStart.defaultPrevented).toBe(true);
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
    expect(dragMark()).toBe('refused');
    await dropColumnOn(root, 'idea', 'right');
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
    await dropColumnOn(root, 'idea', 'left');
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

  /**
   * Lay out an 800px row of 272px columns 12px apart, which scrolls: Doing,
   * the third, spans 568 to 840, and Done starts at 852, past the row's
   * edge. Holds the frames the drag's scroll loop asks for until `runFrame`.
   * The caller unstubs the globals.
   */
  function layOutScrollingRow(root: ParentNode) {
    const row = root.querySelector<HTMLElement>('.board-columns')!;
    let scrollLeft = 0;
    Object.defineProperty(row, 'scrollLeft', {
      configurable: true,
      get: () => scrollLeft,
      set: (value: number) => (scrollLeft = value),
    });
    const rect = (left: number, width: number) =>
      ({ left, right: left + width, top: 0, bottom: 300, width, height: 300 }) as DOMRect;
    row.getBoundingClientRect = () => rect(0, 800);
    for (const child of row.children) {
      child.getBoundingClientRect = () =>
        rect([...row.children].indexOf(child) * 284 - scrollLeft, 272);
    }
    const frames = new Map<number, FrameRequestCallback>();
    let frameId = 0;
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      frames.set(++frameId, callback);
      return frameId;
    });
    vi.stubGlobal('cancelAnimationFrame', (id: number) => void frames.delete(id));
    const runFrame = (time: number) => {
      const due = [...frames.values()];
      frames.clear();
      for (const callback of due) callback(time);
      flushSync();
    };
    return { scrolled: () => scrollLeft, runFrame };
  }

  it('scrolls the row while a column is held near its end, and the line follows', async () => {
    const root = await mountBoard();
    const { scrolled, runFrame } = layOutScrollingRow(root);
    try {
      // Idea, held 10px short of the row's right edge, inside its 48px
      // scroll zone and over Doing's right half.
      const held = { clientX: 790, clientY: 20 };
      pressColumn(root, 'idea');
      movePointer(held);
      expect(dropLines(root)).toEqual(['doing:after']);

      // The first frame only starts the clock.
      runFrame(0);
      expect(scrolled()).toBe(0);
      // 38px into the zone is 712.5px a second: 71px in 100ms.
      runFrame(100);
      expect(scrolled()).toBe(71);
      // The pointer has not moved. Done, now from 781, has come under it.
      expect(dropLines(root)).toEqual(['done:before']);
      expect(dragMark()).toBe('active');

      await releasePointer(held);
      expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);
      expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'idea', 'done']);

      // The release ended the scrolling with the drag.
      runFrame(200);
      expect(scrolled()).toBe(71);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  // A wheel or trackpad scroll moves the columns under a held pointer that
  // sends no move of its own. The line and the cursor have to follow, or
  // they describe one place while the release lands in another.

  it('follows the row as the wheel scrolls it under a held column', async () => {
    const root = await mountBoard();
    layOutScrollingRow(root);
    try {
      const row = root.querySelector<HTMLElement>('.board-columns')!;
      const wheelTo = (scrollLeft: number) => {
        row.scrollLeft = scrollLeft;
        // As the engines send it: from the row, and it does not bubble.
        row.dispatchEvent(new Event('scroll'));
        flushSync();
      };
      // Idea, held over Doing's left half, which is where Idea already is.
      // Clear of both scroll zones, so the drag scrolls nothing itself.
      const held = { clientX: 620, clientY: 20 };
      pressColumn(root, 'idea');
      movePointer(held);
      expect(dropLines(root)).toEqual([]);
      expect(dragMark()).toBe('refused');

      // Doing, now from 468 to 740, has its right half under the pointer.
      wheelTo(100);
      expect(dropLines(root)).toEqual(['doing:after']);
      expect(dragMark()).toBe('active');

      // Done, now from 452 to 724, has its right half under the pointer.
      wheelTo(400);
      expect(dropLines(root)).toEqual(['done:after']);

      // And back to where the drag began.
      wheelTo(0);
      expect(dropLines(root)).toEqual([]);
      expect(dragMark()).toBe('refused');

      wheelTo(400);
      await releasePointer(held);
      expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 2]]);
      expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'doing', 'done', 'idea']);

      // The drag is over: a scroll resolves nothing.
      wheelTo(100);
      expect(dropLines(root)).toEqual([]);
      expect(dragMark()).toBeNull();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  // The row scrolls for a pointer exactly where the board still lands the
  // column: never at full speed under a cursor that says not-allowed for
  // being away from the row.

  it('scrolls the row for a pointer just past its end or above it, with a line to land on', async () => {
    const root = await mountBoard();
    const { scrolled, runFrame } = layOutScrollingRow(root);
    try {
      // Past the row's right edge (800) and above its top, both within reach.
      pressColumn(root, 'idea');
      movePointer({ clientX: 799 + DRAG_REACH, clientY: -DRAG_REACH });
      // The row's last pixel is in Doing's right half.
      expect(dropLines(root)).toEqual(['doing:after']);
      expect(dragMark()).toBe('active');

      runFrame(0);
      // Full speed past the edge: 90px in 100ms.
      runFrame(100);
      expect(scrolled()).toBe(90);
      // Done, now from 762, is what shows at the row's end.
      expect(dropLines(root)).toEqual(['done:before']);
      expect(dragMark()).toBe('active');
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it.each([
    ['past its end', { clientX: 800 + DRAG_REACH, clientY: 20 }],
    ['above it', { clientX: 790, clientY: -1 - DRAG_REACH }],
    ['below it', { clientX: 790, clientY: 300 + DRAG_REACH }],
  ])(
    'does not scroll the row for a pointer clearly %s, where a drop is refused',
    async (_where, away) => {
      const root = await mountBoard();
      const { scrolled, runFrame } = layOutScrollingRow(root);
      try {
        pressColumn(root, 'idea');
        movePointer(away);
        expect(dropLines(root)).toEqual([]);
        expect(dragMark()).toBe('refused');

        runFrame(0);
        runFrame(100);
        runFrame(200);
        expect(scrolled()).toBe(0);
        expect(dragMark()).toBe('refused');

        // Back within reach the row scrolls, and the cursor says so.
        movePointer({ clientX: 790, clientY: 20 });
        expect(dragMark()).toBe('active');
        runFrame(300);
        expect(scrolled()).toBeGreaterThan(0);
      } finally {
        vi.unstubAllGlobals();
      }
    }
  );

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
      expect(columnHandle(root, 'idea').classList.contains('draggable')).toBe(false);

      // Even with a mouse plugged in.
      dragColumnOver(root, 'idea', 'done', 'right');
      expect(dragMark()).toBeNull();
      expect(dropLines(root)).toEqual([]);
      expect(column(root, 'idea').classList.contains('dragging')).toBe(false);
      await dropColumnOn(root, 'done', 'right');

      expect(commandsMock.moveTag).not.toHaveBeenCalled();
    });
  });

  it('does not drag a column under a finger, where the pointer is otherwise fine', async () => {
    const root = await mountBoard();
    layOutColumns(root);

    pressColumn(root, 'idea', { pointerType: 'touch' });
    pointerEvent('pointermove', document.body, pointerOver(root, 'done', 'right'), {
      pointerType: 'touch',
    });
    flushSync();

    expect(dragMark()).toBeNull();
    expect(column(root, 'idea').classList.contains('dragging')).toBe(false);
    pointerEvent('pointerup', document.body, pointerOver(root, 'done', 'right'), {
      pointerType: 'touch',
    });
    await settle();
    expect(commandsMock.moveTag).not.toHaveBeenCalled();
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

describe('ProjectsBoard under the window sort', () => {
  // The projects are p1, p2 and p3, and Priority's tags happen to be named
  // p0 and p1: `{ projectId: 'p3', tagId: 'p0' }` gives project p3 priority P0.
  const BY_PRIORITY = [{ kind: 'group' as const, groupId: 'priority', direction: 'asc' as const }];

  /** The Status board over the window's sorted, filtered list, as ProjectsList wires it. */
  async function mountSorted(state: TagState) {
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
    setProjectSort([]);
  });

  it('orders the cards in each column by the sort, and by the list with none', async () => {
    const root = await mountSorted(
      tagState([
        { projectId: 'p1', tagId: 'doing' },
        { projectId: 'p2', tagId: 'doing' },
        { projectId: 'p2', tagId: 'p1' },
        { projectId: 'p3', tagId: 'doing' },
        { projectId: 'p3', tagId: 'p0' },
      ])
    );
    expect(cardsIn(root, 'doing')).toEqual(['p1', 'p2', 'p3']);

    setProjectSort(BY_PRIORITY);
    await settle();
    // P0, then P1, then the project with no priority.
    expect(cardsIn(root, 'doing')).toEqual(['p3', 'p2', 'p1']);
    expect(setStoreValue).toHaveBeenLastCalledWith('projects-sort', BY_PRIORITY);

    setProjectSort([{ ...BY_PRIORITY[0], direction: 'desc' }]);
    await settle();
    expect(cardsIn(root, 'doing')).toEqual(['p2', 'p3', 'p1']);

    setProjectSort([]);
    await settle();
    expect(cardsIn(root, 'doing')).toEqual(['p1', 'p2', 'p3']);
  });

  it('lands a card dropped on another column in its sorted slot', async () => {
    const root = await mountSorted(
      tagState([
        { projectId: 'p1', tagId: 'doing' },
        { projectId: 'p2', tagId: 'doing' },
        { projectId: 'p2', tagId: 'p1' },
        { projectId: 'p3', tagId: 'idea' },
        { projectId: 'p3', tagId: 'p0' },
      ])
    );
    setProjectSort(BY_PRIORITY);
    await settle();
    expect(cardsIn(root, 'doing')).toEqual(['p2', 'p1']);
    expect(cardsIn(root, 'idea')).toEqual(['p3']);

    // The drop picks the column; the sort picks the slot, here the top.
    await dragTo(root, 'p3', over(column(root, 'doing')));

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p3', 'status', 'doing');
    expect(cardsIn(root, 'doing')).toEqual(['p3', 'p2', 'p1']);
    expect(cardsIn(root, 'idea')).toEqual([]);
    expect(document.activeElement).toBe(card(root, 'p3'));
  });

  it('a key for the board’s own group changes nothing, and the next key decides', async () => {
    const root = await mountSorted(
      tagState([
        { projectId: 'p1', tagId: 'doing' },
        { projectId: 'p2', tagId: 'doing' },
        { projectId: 'p2', tagId: 'p0' },
        { projectId: 'p3', tagId: 'idea' },
      ])
    );
    const byStatus = { kind: 'group' as const, groupId: 'status', direction: 'desc' as const };

    // Every card in a column has the same Status, so the key ties throughout.
    setProjectSort([byStatus]);
    await settle();
    expect(columnKeys(root)).toEqual([NO_TAG_COLUMN_KEY, 'idea', 'doing', 'done']);
    expect(cardsIn(root, 'doing')).toEqual(['p1', 'p2']);
    expect(cardsIn(root, 'idea')).toEqual(['p3']);

    setProjectSort([byStatus, ...BY_PRIORITY]);
    await settle();
    expect(cardsIn(root, 'doing')).toEqual(['p2', 'p1']);
    expect(cardsIn(root, 'idea')).toEqual(['p3']);
  });
});
