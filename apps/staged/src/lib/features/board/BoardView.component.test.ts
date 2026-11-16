// Mounts the pinned board's detail view over the real tagsStore, filter
// store and board, with the backend commands, the data stores and
// navigation mocked: it renders the route's group without the "Board by"
// picker, opens a card's project, offers the window's sort menu with its own
// group's key marked as doing nothing, and leaves the board route once the
// group is unpinned or deleted (navigation.test.ts covers where that lands).
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, TagState } from '../../types';
import { tagState } from '../tags/testStubs/tagFixtures';

const { backend, navigationMock } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  navigationMock: {
    leaveBoard: vi.fn(),
    selectProject: vi.fn(),
    openSettings: vi.fn(),
  },
}));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({ data: structuredClone(backend.state), revalidating: null })),
  setProjectGroupTag: vi.fn(async () => {}),
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
  setStoreValue: vi.fn(async () => {}),
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    get liveProjects() {
      return projects;
    },
    get projects() {
      return projects;
    },
    loading: false,
    loaded: true,
    error: null,
    branchesByProject: new Map(),
    reposByProject: new Map(),
    deletingProjectNames: new Map(),
    transferringProjects: new Map(),
    isProjectDeleting: () => false,
    isProjectHydrated: () => true,
    ensureLoaded: vi.fn(async () => {}),
    ensureProjectsHydrated: vi.fn(async () => {}),
    projectCreated: vi.fn(),
  },
}));
vi.mock('../../stores/projectState.svelte', () => ({
  projectStateStore: { isUnread: () => false },
}));
vi.mock('../../stores/repoBadges.svelte', () => ({ repoBadgeStore: { lookup: () => null } }));
vi.mock('../projects/projectActions.svelte', () => ({
  projectActions: { markProjectUnread: vi.fn(), requestRemoveProject: vi.fn() },
}));
vi.mock('../projects/projectStatus', () => ({
  getProjectStatus: () => ({ kind: 'idle', runningCount: 0, runActionPhase: null }),
}));
vi.mock('../projects/workspaceLifecycle.svelte', () => ({
  workspaceLifecycle: { getWorktreeErrors: () => new Map() },
}));
vi.mock('../projects/projectSearchIndex.svelte', () => ({
  projectSearchIndex: { index: null, ensureLoaded: vi.fn() },
}));
vi.mock(
  '../projects/ProjectTransferMenuItem.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../projects/ProjectCardLabels.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../projects/ProjectCardStatusIndicator.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../projects/NewProjectModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../layout/TopBarPortal.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('../layout/navigation.svelte', () => ({
  ...navigationMock,
  onViewHidden: () => {},
  followWhileVisible: <T>(read: () => T) => ({
    get current() {
      return read();
    },
  }),
}));
vi.mock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));

import { tagsStore } from '../../stores/tags.svelte';
import { projectFiltersStore } from '../projects/projectFilters.svelte';
import { projectSortPrefs } from '../projects/projectSortPrefs.svelte';
import { projectsListViewState } from '../projects/projectsListViewState.svelte';
import BoardView from './BoardView.svelte';

let component: ReturnType<typeof mount> | null = null;

async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

/** The fixture catalog with Status's board pinned. */
function pinnedStatus(assignments: TagState['assignments'] = []): TagState {
  const state = tagState(assignments);
  return {
    ...state,
    groups: state.groups.map((g) => (g.id === 'status' ? { ...g, showInSidebar: true } : g)),
  };
}

async function mountView(state: TagState, groupId = 'status') {
  backend.state = state;
  await tagsStore.load({ force: true });
  projects.splice(0, projects.length, project('p1'), project('p2'), project('p3'));
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(BoardView, { target, props: { groupId } });
  await settle();
  return target;
}

async function echo(state: TagState) {
  backend.state = state;
  await tagsStore.load({ force: true });
  await settle();
}

function columnKeys(root: ParentNode): string[] {
  return [...root.querySelectorAll<HTMLElement>('[data-board-column]')].map(
    (el) => el.dataset.boardColumn!
  );
}

function sortTrigger(root: ParentNode): HTMLElement {
  const found = root.querySelector<HTMLElement>('.search-row [aria-label^="Sort projects"]');
  if (!found) throw new Error('No sort menu in the search row');
  return found;
}

/** Open the sort menu and return its key rows as "name: hint", top to bottom. */
async function sortKeyRows(root: ParentNode): Promise<string[]> {
  sortTrigger(root).dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await settle();
  return [...document.querySelectorAll<HTMLElement>('[data-sort-key]')].map((row) => {
    const [, name, hint] = [...row.querySelectorAll('span')].map((el) => el.textContent!.trim());
    return `${name}: ${hint}`;
  });
}

function card(root: ParentNode, projectId: string): HTMLElement {
  const found = [...root.querySelectorAll<HTMLElement>('[data-board-card]')].find(
    (el) => el.dataset.boardCard === projectId
  );
  if (!found) throw new Error(`No card ${projectId}`);
  return found;
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
  projectFiltersStore.clearFilters();
  projectsListViewState.searchQuery = '';
  projectSortPrefs.sort = [];
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('BoardView', () => {
  it("renders the route's group as a board, without the group picker", async () => {
    const root = await mountView(
      pinnedStatus([
        { projectId: 'p1', tagId: 'done' },
        { projectId: 'p3', tagId: 'idea' },
      ])
    );
    expect(root.querySelector('[aria-label="Projects by Status"]')).not.toBeNull();
    expect(columnKeys(root)).toEqual(['__none__', 'idea', 'doing', 'done']);
    expect(card(root, 'p1').closest('[data-board-column]')!.getAttribute('data-board-column')).toBe(
      'done'
    );
    // The group is the route's: no "Board by" trigger, and no Priority board.
    expect(root.textContent).not.toContain('Board by');
    expect(root.querySelector('[aria-label="Projects by Priority"]')).toBeNull();
    expect(navigationMock.leaveBoard).not.toHaveBeenCalled();
  });

  it('shows the board the route names even when another group is pinned too', async () => {
    const state = pinnedStatus();
    const root = await mountView(
      { ...state, groups: state.groups.map((g) => ({ ...g, showInSidebar: true })) },
      'priority'
    );
    expect(root.querySelector('[aria-label="Projects by Priority"]')).not.toBeNull();
    expect(columnKeys(root)).toEqual(['__none__', 'p0', 'p1']);
  });

  it('opens a project from its card', async () => {
    const root = await mountView(pinnedStatus([{ projectId: 'p1', tagId: 'done' }]));
    card(root, 'p1').click();
    expect(navigationMock.selectProject).toHaveBeenCalledWith('p1');
  });

  it('narrows the cards with the filter chips, as the home board does', async () => {
    const root = await mountView(
      pinnedStatus([
        { projectId: 'p1', tagId: 'done' },
        { projectId: 'p2', tagId: 'q4' },
        { projectId: 'p3', tagId: 'idea' },
      ])
    );
    projectFiltersStore.toggleFilter({ tagId: 'q4' });
    await settle();
    const shown = [...root.querySelectorAll<HTMLElement>('[data-board-card]')].map(
      (el) => el.dataset.boardCard
    );
    expect(shown).toEqual(['p2']);
  });

  it('offers the sort menu in its search row, naming the window’s sort', async () => {
    projectSortPrefs.sort = [{ kind: 'group', groupId: 'priority', direction: 'asc' }];
    const root = await mountView(pinnedStatus());

    expect(sortTrigger(root).getAttribute('aria-label')).toBe('Sort projects: Priority');
    expect(await sortKeyRows(root)).toEqual(['Priority: Tag order']);
  });

  it('marks the key for its own group as used for the columns', async () => {
    projectSortPrefs.sort = [
      { kind: 'group', groupId: 'status', direction: 'asc' },
      { kind: 'group', groupId: 'priority', direction: 'desc' },
    ];
    const root = await mountView(pinnedStatus());

    expect(await sortKeyRows(root)).toEqual([
      'Status: Used for columns here',
      'Priority: Reverse tag order',
    ]);
    // Still named on the trigger: it orders the grid and the sidebar.
    expect(sortTrigger(root).getAttribute('aria-label')).toBe('Sort projects: Status, Priority');
  });

  it('marks the group of the board the route names, not another pinned one', async () => {
    projectSortPrefs.sort = [
      { kind: 'group', groupId: 'status', direction: 'asc' },
      { kind: 'group', groupId: 'priority', direction: 'asc' },
    ];
    const state = pinnedStatus();
    const root = await mountView(
      { ...state, groups: state.groups.map((g) => ({ ...g, showInSidebar: true })) },
      'priority'
    );

    expect(await sortKeyRows(root)).toEqual([
      'Status: Tag order',
      'Priority: Used for columns here',
    ]);
  });

  it('sorts its cards by the key the menu marks, once past its own group', async () => {
    // Priority then Status on the Priority board: Priority ties in every
    // column, so Status orders the cards.
    projectSortPrefs.sort = [
      { kind: 'group', groupId: 'priority', direction: 'asc' },
      { kind: 'group', groupId: 'status', direction: 'asc' },
    ];
    const state = pinnedStatus([
      { projectId: 'p1', tagId: 'done' },
      { projectId: 'p2', tagId: 'idea' },
      { projectId: 'p3', tagId: 'doing' },
    ]);
    const root = await mountView(
      { ...state, groups: state.groups.map((g) => ({ ...g, showInSidebar: true })) },
      'priority'
    );

    const shown = [...root.querySelectorAll<HTMLElement>('[data-board-card]')].map(
      (el) => el.dataset.boardCard
    );
    expect(shown).toEqual(['p2', 'p3', 'p1']);
  });

  it('leaves the board route once its board is unpinned', async () => {
    await mountView(pinnedStatus());
    await echo(tagState());
    expect(navigationMock.leaveBoard).toHaveBeenCalledTimes(1);
    expect(navigationMock.leaveBoard).toHaveBeenCalledWith('status');
  });

  it('leaves the board route once its group is deleted', async () => {
    await mountView(pinnedStatus());
    const state = pinnedStatus();
    await echo({
      ...state,
      groups: state.groups.filter((g) => g.id !== 'status'),
      tags: state.tags.filter((t) => t.groupId !== 'status'),
    });
    expect(navigationMock.leaveBoard).toHaveBeenCalledTimes(1);
    expect(navigationMock.leaveBoard).toHaveBeenCalledWith('status');
  });

  it('stays while another pinned board is unpinned', async () => {
    const state = pinnedStatus();
    await mountView({ ...state, groups: state.groups.map((g) => ({ ...g, showInSidebar: true })) });
    await echo(pinnedStatus());
    expect(navigationMock.leaveBoard).not.toHaveBeenCalled();
  });
});
