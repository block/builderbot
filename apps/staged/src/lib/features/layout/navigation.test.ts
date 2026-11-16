import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiffDetailRoute } from './navigation.svelte';

// ── Mock plumbing ──

let windowLabel: string | null;
let getStoreValue: ReturnType<typeof vi.fn>;
let initPersistentStore: ReturnType<typeof vi.fn>;
let setStoreValue: ReturnType<typeof vi.fn>;
let writeSnapshot: ReturnType<typeof vi.fn>;
let clearSnapshot: ReturnType<typeof vi.fn>;
let snapshotProjectId: string | null;
let projectsStoreMock: {
  projects: Array<{ id: string }>;
  loaded: boolean;
  ensureLoaded: ReturnType<typeof vi.fn>;
  whenLoaded: ReturnType<typeof vi.fn>;
  isProjectDeleting: (projectId: string) => boolean;
};
let deletingProjectIds: Set<string>;
/** The window's sorted project list (projectOrder), which next and previous walk. */
let projectOrderMock: { sortedProjects: Array<{ id: string }> };
let takeWindowSeed: ReturnType<typeof vi.fn>;
let pruneDeletedProjects: ReturnType<typeof vi.fn>;
/** The tag catalog, as far as navigation reads it: which boards are pinned. */
let tagsStoreMock: {
  loaded: boolean;
  catalog: { groupsById: Map<string, { showInSidebar: boolean }> };
};
/** Whether the window has the mobile layout (no sidebar) as it opens. */
let viewportMock: { isMobile: boolean };
/** What the backend says about a diff entry a step is about to land on.
 *  By default: it is as it was opened. That answer is a new route object,
 *  never the one asked about, as the real refreshDiffRoute's always is. */
let refreshDiffRoute: ReturnType<typeof vi.fn>;

async function importNavigation() {
  return await import('./navigation.svelte');
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (agent.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);

  windowLabel = 'main';
  getStoreValue = vi.fn().mockResolvedValue(null);
  initPersistentStore = vi.fn().mockResolvedValue(undefined);
  setStoreValue = vi.fn().mockResolvedValue(undefined);
  writeSnapshot = vi.fn();
  clearSnapshot = vi.fn();
  snapshotProjectId = null;
  deletingProjectIds = new Set();
  projectOrderMock = { sortedProjects: [] };
  projectsStoreMock = {
    projects: [],
    loaded: false,
    ensureLoaded: vi.fn().mockResolvedValue(undefined),
    whenLoaded: vi.fn().mockResolvedValue(undefined),
    isProjectDeleting: (projectId) => deletingProjectIds.has(projectId),
  };
  takeWindowSeed = vi.fn().mockResolvedValue(null);
  pruneDeletedProjects = vi.fn().mockResolvedValue(undefined);
  tagsStoreMock = { loaded: false, catalog: { groupsById: new Map() } };
  viewportMock = { isMobile: false };
  refreshDiffRoute = vi.fn(async (route: DiffDetailRoute) => ({
    status: 'current',
    route: { ...route },
  }));

  vi.doMock('../../transport', () => ({
    getWindowLabel: () => windowLabel,
  }));
  vi.doMock('../../shared/persistentStore', () => ({
    getStoreValue,
    initPersistentStore,
    setStoreValue,
  }));
  vi.doMock('../../shared/webSnapshot', () => ({
    SNAPSHOT_KEYS: { lastProject: 'staged:boot:last-project' },
    readSnapshot: () => snapshotProjectId,
    writeSnapshot,
    clearSnapshot,
  }));
  vi.doMock('../../commands', () => ({ takeWindowSeed }));
  vi.doMock('../../stores/projectState.svelte', () => ({
    projectStateStore: {
      isUnread: () => false,
      markAsRead: vi.fn(),
      pruneDeletedProjects,
    },
  }));
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: projectsStoreMock,
  }));
  vi.doMock('../projects/projectsListViewState.svelte', () => ({
    requestProjectsListRestore: vi.fn(),
  }));
  vi.doMock('../projects/projectOrder.svelte', () => ({ projectOrderStore: projectOrderMock }));
  vi.doMock('../../stores/tags.svelte', () => ({ tagsStore: tagsStoreMock }));
  vi.doMock('../../shared/viewport.svelte', () => ({ viewport: viewportMock }));
  vi.doMock('./diffRouteRefresh', () => ({ refreshDiffRoute }));
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.resetAllMocks();
});

describe('restoreInitialRoute', () => {
  it('opens on the stored project before settling on an untouched Tauri history', async () => {
    windowLabel = 'main';
    getStoreValue.mockResolvedValueOnce('p1');
    const { restoreInitialRoute, navigation } = await importNavigation();
    expect(navigation.initialRouteSettled).toBe(false);

    await restoreInitialRoute();

    expect(initPersistentStore).toHaveBeenCalledTimes(1);
    expect(getStoreValue).toHaveBeenCalledWith('last-viewed-project');
    expect(navigation.detailStack).toEqual([{ kind: 'project', projectId: 'p1' }]);
    expect(navigation.selectedProjectId).toBe('p1');
    // The window opened on the project: the grid was never visited, so it is
    // not a step Back.
    expect(navigation.history.entries).toHaveLength(1);
    expect(navigation.canGoBack).toBe(false);
    expect(navigation.canGoForward).toBe(false);
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('settles at the root when the stored value is null', async () => {
    getStoreValue.mockResolvedValueOnce(null);
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.selectedProjectId).toBeNull();
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('settles on timeout when the store read hangs after init', async () => {
    vi.useFakeTimers();
    getStoreValue.mockReturnValueOnce(new Promise(() => {}));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { restoreInitialRoute, navigation, INITIAL_ROUTE_TIMEOUT_MS } = await importNavigation();

    const restore = restoreInitialRoute();
    await vi.advanceTimersByTimeAsync(INITIAL_ROUTE_TIMEOUT_MS - 1);
    expect(navigation.initialRouteSettled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await restore;

    expect(navigation.initialRouteSettled).toBe(true);
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('waits out a slow store init and still restores the route', async () => {
    // The realistic degradation: preferences_store_path + load() are slow
    // (cold disk, remote web), but the key read is in-memory once init lands.
    // `<main>` is gated on preferences.loaded, which waits on the same init,
    // so the timeout must not start until init resolves.
    vi.useFakeTimers();
    let resolveInit!: () => void;
    initPersistentStore.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        resolveInit = resolve;
      })
    );
    getStoreValue.mockResolvedValueOnce('p1');
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { restoreInitialRoute, navigation, INITIAL_ROUTE_TIMEOUT_MS } = await importNavigation();

    const restore = restoreInitialRoute();
    await vi.advanceTimersByTimeAsync(INITIAL_ROUTE_TIMEOUT_MS * 3);
    // Init is still pending: nothing read, nothing settled, no timeout fired.
    expect(getStoreValue).not.toHaveBeenCalled();
    expect(navigation.initialRouteSettled).toBe(false);
    expect(warn).not.toHaveBeenCalled();

    resolveInit();
    await restore;

    expect(getStoreValue).toHaveBeenCalledWith('last-viewed-project');
    expect(navigation.detailStack).toEqual([{ kind: 'project', projectId: 'p1' }]);
    expect(navigation.initialRouteSettled).toBe(true);
    expect(warn).not.toHaveBeenCalled();
  });

  it('settles when the store read rejects', async () => {
    getStoreValue.mockRejectedValueOnce(new Error('store down'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(navigation.initialRouteSettled).toBe(true);
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('leaves a web-snapshot-seeded history alone', async () => {
    snapshotProjectId = 'p1';
    windowLabel = null;
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(initPersistentStore).not.toHaveBeenCalled();
    expect(getStoreValue).not.toHaveBeenCalled();
    expect(navigation.detailStack).toEqual([{ kind: 'project', projectId: 'p1' }]);
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('leaves secondary windows unchanged', async () => {
    windowLabel = 'win-2';
    getStoreValue.mockResolvedValueOnce('p1');
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(initPersistentStore).not.toHaveBeenCalled();
    expect(getStoreValue).not.toHaveBeenCalled();
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.initialRouteSettled).toBe(true);
    // The grid is this window's base route until a seed lands, so it must be
    // retained here or a window with no seed renders two empty layers.
    expect(navigation.homeLayerRetained).toBe(true);
  });

  // A secondary window settles at the root before initNavigation reads its
  // seed. If the seed never lands the grid is all the window has, so the
  // settle must have retained it: nothing at the root triggers a goHome that
  // would.
  it.each([
    ['rejects', () => takeWindowSeed.mockRejectedValueOnce(new Error('no seed'))],
    ['is null', () => takeWindowSeed.mockResolvedValueOnce(null)],
    [
      'names a deleted project',
      () => {
        takeWindowSeed.mockResolvedValueOnce('gone');
        projectsStoreMock.loaded = true;
        projectsStoreMock.projects = [{ id: 'p1' }];
      },
    ],
  ])('keeps the grid for a secondary window whose seed %s', async (_, arrange) => {
    windowLabel = 'win-2';
    arrange();
    const { restoreInitialRoute, initNavigation, navigation } = await importNavigation();

    await restoreInitialRoute();
    await initNavigation();

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.homeActive).toBe(true);
    expect(navigation.initialRouteSettled).toBe(true);
    expect(navigation.homeLayerRetained).toBe(true);
  });

  it('does not override navigation that happened while the store was read', async () => {
    let resolveRead!: (value: string) => void;
    getStoreValue.mockReturnValueOnce(
      new Promise<string>((resolve) => {
        resolveRead = resolve;
      })
    );
    const { restoreInitialRoute, selectProject, navigation } = await importNavigation();

    const restore = restoreInitialRoute();
    selectProject('p2');
    resolveRead('p1');
    await restore;

    expect(navigation.selectedProjectId).toBe('p2');
    expect(navigation.initialRouteSettled).toBe(true);
  });

  it('hands a restored project to initNavigation as a seeded history', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p2' }];
    const { restoreInitialRoute, initNavigation, navigation } = await importNavigation();

    await restoreInitialRoute();
    await initNavigation();

    // Validation reused the restored route rather than re-reading the store,
    // and backed out because the project is gone.
    expect(getStoreValue).toHaveBeenCalledTimes(1);
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', null);
  });
});

describe('initial project restore', () => {
  it('restores the synchronous snapshot in the first window and web mode', async () => {
    snapshotProjectId = 'persisted';
    windowLabel = 'main';
    let module = await importNavigation();
    expect(module.navigation.selectedProjectId).toBe('persisted');
    expect(module.navigation.canGoBack).toBe(false);
    expect(module.navigation.detailStack).toEqual([{ kind: 'project', projectId: 'persisted' }]);
    expect(module.navigation.currentRoute).toEqual({ kind: 'project', projectId: 'persisted' });

    vi.resetModules();
    windowLabel = null;
    module = await importNavigation();
    expect(module.navigation.selectedProjectId).toBe('persisted');
    expect(module.navigation.canGoBack).toBe(false);
    expect(module.navigation.detailStack).toEqual([{ kind: 'project', projectId: 'persisted' }]);
  });

  it('starts a secondary window empty instead of leaking the shared snapshot', async () => {
    snapshotProjectId = 'persisted';
    windowLabel = 'win-2';

    const { navigation } = await importNavigation();

    expect(navigation.selectedProjectId).toBeNull();
    expect(navigation.canGoBack).toBe(false);
    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
  });
});

describe('persistLastProject', () => {
  it('writes the legacy unsuffixed key in the first window', async () => {
    windowLabel = 'main';
    const { selectProject } = await importNavigation();

    selectProject('p1');

    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', 'p1');
    expect(writeSnapshot).toHaveBeenCalledWith('staged:boot:last-project', 'p1');
  });

  it('writes the legacy unsuffixed key in web mode (no window label)', async () => {
    windowLabel = null;
    const { selectProject } = await importNavigation();

    selectProject('p1');

    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', 'p1');
  });

  it('persists nothing in a secondary window', async () => {
    windowLabel = 'win-2';
    const { selectProject, goHome } = await importNavigation();

    // Navigating to a project and back home both persist in `main`; neither may
    // write a `…:win-2` key nobody ever reads.
    selectProject('p1');
    goHome();

    expect(setStoreValue).not.toHaveBeenCalled();
    expect(writeSnapshot).not.toHaveBeenCalled();
    expect(clearSnapshot).not.toHaveBeenCalled();
  });

  it('still navigates in a secondary window', async () => {
    windowLabel = 'win-2';
    const { selectProject, navigation } = await importNavigation();

    selectProject('p1');

    expect(navigation.selectedProjectId).toBe('p1');
  });
});

describe('initNavigation', () => {
  it('leaves a seeded web history untouched when the project still exists', async () => {
    snapshotProjectId = 'p1';
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(projectsStoreMock.whenLoaded).toHaveBeenCalledTimes(1);
    expect(projectsStoreMock.ensureLoaded).not.toHaveBeenCalled();
    expect(navigation.detailStack).toEqual([{ kind: 'project', projectId: 'p1' }]);
    expect(navigation.history.entries).toHaveLength(1);
    expect(navigation.canGoBack).toBe(false);
  });

  it('resets a seeded history and clears persistence when the restored project is gone', async () => {
    snapshotProjectId = 'p1';
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p2' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.selectedProjectId).toBeNull();
    // The missing project is not left behind as a step Back.
    expect(navigation.history.entries).toEqual([[{ kind: 'projects' }]]);
    expect(navigation.canGoBack).toBe(false);
    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', null);
    expect(clearSnapshot).toHaveBeenCalledWith('staged:boot:last-project');
  });

  it('does not undo a goHome that happens before validation resolves', async () => {
    snapshotProjectId = 'p1';
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const pending = new Promise<void>((resolve) => setTimeout(resolve, 0));
    projectsStoreMock.whenLoaded.mockReturnValueOnce(pending);
    const { initNavigation, goHome, navigation } = await importNavigation();

    const restore = initNavigation();
    goHome();
    await restore;

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(navigation.selectedProjectId).toBeNull();
    // And the project the window opened on is still a step Back.
    expect(navigation.history.entries).toEqual([
      [{ kind: 'project', projectId: 'p1' }],
      [{ kind: 'projects' }],
    ]);
  });

  it('opens on the stored project on a pristine Tauri history', async () => {
    snapshotProjectId = null;
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([{ kind: 'project', projectId: 'p1' }]);
    expect(navigation.selectedProjectId).toBe('p1');
    expect(navigation.canGoBack).toBe(false);
  });

  it('clears a missing stored Tauri project even from a pristine history', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p2' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([{ kind: 'projects' }]);
    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', null);
    expect(clearSnapshot).toHaveBeenCalledWith('staged:boot:last-project');
  });

  it('does not clobber user navigation while validating a stored Tauri project', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }, { id: 'p2' }];
    const pending = new Promise<void>((resolve) => setTimeout(resolve, 0));
    projectsStoreMock.whenLoaded.mockReturnValueOnce(pending);
    const { initNavigation, selectProject, navigation } = await importNavigation();

    const restore = initNavigation();
    selectProject('p2');
    await restore;

    expect(navigation.detailStack).toEqual([{ kind: 'project', projectId: 'p2' }]);
    expect(navigation.selectedProjectId).toBe('p2');
    // The stored project was not restored over, or under, where the user went.
    expect(navigation.history.entries).toEqual([
      [{ kind: 'projects' }],
      [{ kind: 'project', projectId: 'p2' }],
    ]);
  });

  it('opens on a valid secondary-window seed after validation', async () => {
    windowLabel = 'win-2';
    takeWindowSeed.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([{ kind: 'project', projectId: 'p1' }]);
    expect(navigation.selectedProjectId).toBe('p1');
    expect(navigation.canGoBack).toBe(false);
  });
});

// The mobile layout has no sidebar, so a window that opens on a project
// there has the grid one step Back. With a sidebar (every test above) the
// project is the only entry.
describe('a window that opens on a project in the mobile layout', () => {
  const home = { kind: 'projects' };
  const p1 = { kind: 'project', projectId: 'p1' };

  beforeEach(() => {
    viewportMock.isMobile = true;
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    vi.stubGlobal('document', { querySelectorAll: () => [] });
  });

  it('puts the grid under a project restored from the store', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    const { restoreInitialRoute, goBack, goForward, navigation } = await importNavigation();

    await restoreInitialRoute();

    // On the project, as on desktop, with the grid one step Back.
    expect(navigation.detailStack).toEqual([p1]);
    expect(navigation.selectedProjectId).toBe('p1');
    expect(navigation.homeActive).toBe(false);
    expect(navigation.history.entries).toEqual([[home], [p1]]);
    expect(navigation.canGoBack).toBe(true);
    expect(navigation.canGoForward).toBe(false);
    // Opening is not a navigation: nothing was persisted.
    expect(setStoreValue).not.toHaveBeenCalled();

    await expect(goBack()).resolves.toBe(true);
    expect(navigation.detailStack).toEqual([home]);
    expect(navigation.homeActive).toBe(true);
    expect(navigation.homeLayerRetained).toBe(true);
    expect(navigation.canGoBack).toBe(false);
    expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', null);

    await expect(goForward()).resolves.toBe(true);
    expect(navigation.detailStack).toEqual([p1]);
  });

  it('puts the grid under a project seeded from the web snapshot', async () => {
    snapshotProjectId = 'p1';
    windowLabel = null;
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { restoreInitialRoute, initNavigation, navigation } = await importNavigation();

    // In place from the first frame, before anything is restored.
    expect(navigation.detailStack).toEqual([p1]);
    expect(navigation.history.entries).toEqual([[home], [p1]]);
    expect(navigation.canGoBack).toBe(true);
    // The grid has not been shown, so it is not mounted under the project.
    expect(navigation.homeLayerRetained).toBe(false);

    await restoreInitialRoute();
    await initNavigation();

    expect(navigation.history.entries).toEqual([[home], [p1]]);
    expect(navigation.history.index).toBe(1);
    expect(navigation.homeLayerRetained).toBe(false);
  });

  it('puts the grid under a project restored late, by initNavigation', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([p1]);
    expect(navigation.history.entries).toEqual([[home], [p1]]);
    expect(navigation.canGoBack).toBe(true);
  });

  it('puts the grid under the project a narrow second window is seeded with', async () => {
    windowLabel = 'win-2';
    takeWindowSeed.mockResolvedValueOnce('p1');
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p1' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.detailStack).toEqual([p1]);
    expect(navigation.history.entries).toEqual([[home], [p1]]);
  });

  it('starts over at the grid alone when the restored project is gone', async () => {
    snapshotProjectId = 'p1';
    windowLabel = null;
    projectsStoreMock.loaded = true;
    projectsStoreMock.projects = [{ id: 'p2' }];
    const { initNavigation, navigation } = await importNavigation();

    await initNavigation();

    expect(navigation.history.entries).toEqual([[home]]);
    expect(navigation.canGoBack).toBe(false);
    expect(navigation.canGoForward).toBe(false);
  });

  it('opens at the grid alone when there is no project to restore', async () => {
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(navigation.history.entries).toEqual([[home]]);
    expect(navigation.canGoBack).toBe(false);
  });

  it('adds no grid for a project the user opens, only for the one the window opens on', async () => {
    const { restoreInitialRoute, selectProject, navigation } = await importNavigation();
    await restoreInitialRoute();

    selectProject('p1');
    selectProject('p2');

    expect(navigation.history.entries).toEqual([
      [home],
      [p1],
      [{ kind: 'project', projectId: 'p2' }],
    ]);
  });
});

// Back and Forward: the history of views the window has visited. The model
// itself (entries, index, truncation) is covered in navigationHistory.test.ts;
// here it is what each navigation does to it and what a step does to the view.
describe('history navigation', () => {
  const home = { kind: 'projects' };
  const repos = { kind: 'repos' };
  const projectRoute = (projectId: string) => ({ kind: 'project', projectId });
  const settings = (section: string) => ({ kind: 'settings', section });

  let dialogs: Array<{ getAttribute: (name: string) => string | null }>;
  let dispatchEvent: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    dialogs = [];
    dispatchEvent = vi.fn();
    vi.stubGlobal('window', { dispatchEvent });
    vi.stubGlobal('document', { querySelectorAll: () => dialogs });
  });

  it('starts with nothing to go back or forward to', async () => {
    const { goBack, goForward, navigation } = await importNavigation();

    expect(navigation.history.entries).toEqual([[home]]);
    expect(navigation.canGoBack).toBe(false);
    expect(navigation.canGoForward).toBe(false);

    // A step with no entry that way does nothing.
    await expect(goBack()).resolves.toBe(false);
    await expect(goForward()).resolves.toBe(false);
    expect(navigation.detailStack).toEqual([home]);
  });

  it('records each view visited, whatever it was opened from', async () => {
    const { selectProject, showAllRepos, selectBoard, openSettings, goHome, navigation } =
      await importNavigation();

    selectProject('a');
    selectProject('b');
    showAllRepos();
    selectBoard('g1');
    openSettings('general');
    goHome();

    expect(navigation.history.entries).toEqual([
      [home],
      [projectRoute('a')],
      [projectRoute('b')],
      [repos],
      [{ kind: 'board', groupId: 'g1' }],
      [{ kind: 'board', groupId: 'g1' }, settings('general')],
      [home],
    ]);
    expect(navigation.history.index).toBe(6);
    expect(navigation.canGoBack).toBe(true);
    expect(navigation.canGoForward).toBe(false);
  });

  it('steps back to the previous view, not up to the grid', async () => {
    const { selectProject, showAllRepos, goBack, navigation } = await importNavigation();
    selectProject('a');
    selectProject('b');
    showAllRepos();
    selectProject('c');

    await expect(goBack()).resolves.toBe(true);
    expect(navigation.currentRoute).toEqual(repos);
    expect(navigation.showReposList).toBe(true);

    await goBack();
    expect(navigation.currentRoute).toEqual(projectRoute('b'));
    expect(navigation.selectedProjectId).toBe('b');

    await goBack();
    expect(navigation.currentRoute).toEqual(projectRoute('a'));

    await goBack();
    expect(navigation.currentRoute).toEqual(home);
    expect(navigation.homeActive).toBe(true);
    expect(navigation.canGoBack).toBe(false);
    expect(navigation.canGoForward).toBe(true);
  });

  it('steps forward again through the views it stepped back from', async () => {
    const { selectProject, goBack, goForward, navigation } = await importNavigation();
    selectProject('a');
    selectProject('b');
    await goBack();
    await goBack();

    await expect(goForward()).resolves.toBe(true);
    expect(navigation.currentRoute).toEqual(projectRoute('a'));
    expect(navigation.canGoBack).toBe(true);
    expect(navigation.canGoForward).toBe(true);

    await goForward();
    expect(navigation.currentRoute).toEqual(projectRoute('b'));
    expect(navigation.canGoForward).toBe(false);
  });

  it('drops the forward entries when going somewhere new after going back', async () => {
    const { selectProject, goBack, goForward, navigation } = await importNavigation();
    selectProject('a');
    selectProject('b');
    selectProject('c');
    await goBack();
    await goBack();

    selectProject('d');

    expect(navigation.history.entries).toEqual([[home], [projectRoute('a')], [projectRoute('d')]]);
    expect(navigation.canGoForward).toBe(false);
    await expect(goForward()).resolves.toBe(false);
    expect(navigation.currentRoute).toEqual(projectRoute('d'));
  });

  it('adds no entry, and keeps the forward ones, for the view already on screen', async () => {
    const { selectProject, goHome, goBack, navigation } = await importNavigation();
    selectProject('a');
    selectProject('b');
    await goBack();

    selectProject('a');

    expect(navigation.history.entries).toEqual([[home], [projectRoute('a')], [projectRoute('b')]]);
    expect(navigation.canGoForward).toBe(true);

    await goBack();
    goHome();
    expect(navigation.history.entries).toHaveLength(3);
    expect(navigation.canGoForward).toBe(true);
  });

  it('persists the project a step lands on, and none for the grid', async () => {
    const { selectProject, showAllRepos, goBack, goForward } = await importNavigation();
    selectProject('a');
    selectProject('b');
    showAllRepos();
    setStoreValue.mockClear();

    await goBack();
    expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', 'b');
    await goBack();
    expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', 'a');
    await goBack();
    expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', null);
    await goForward();
    expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', 'a');
  });

  it('asks the grid to restore to the project a step back left', async () => {
    const { requestProjectsListRestore } = await import('../projects/projectsListViewState.svelte');
    const { selectProject, goBack } = await importNavigation();
    selectProject('a');

    await goBack();

    expect(requestProjectsListRestore).toHaveBeenCalledWith('a');
  });

  it('does not step while a dialog is open', async () => {
    const { selectProject, goBack, goForward, navigation } = await importNavigation();
    selectProject('a');
    selectProject('b');
    await goBack();
    dialogs.push({ getAttribute: () => 'open' });

    await expect(goBack()).resolves.toBe(false);
    await expect(goForward()).resolves.toBe(false);
    expect(navigation.currentRoute).toEqual(projectRoute('a'));

    // One that is only animating out is not in the way.
    dialogs = [{ getAttribute: () => 'closed' }];
    await expect(goBack()).resolves.toBe(true);
    expect(navigation.currentRoute).toEqual(home);
  });

  describe('Settings', () => {
    it('is one entry however many sections are looked at', async () => {
      const { selectProject, openSettings, goBack, goForward, navigation } =
        await importNavigation();
      selectProject('a');

      openSettings('general');
      openSettings('keyboard');
      openSettings('doctor');

      expect(navigation.history.entries).toEqual([
        [home],
        [projectRoute('a')],
        [projectRoute('a'), settings('doctor')],
      ]);

      // Back leaves Settings for the project it was opened from, and Forward
      // reopens it on the section last looked at.
      await goBack();
      expect(navigation.activeView).toBe('workspace');
      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      await goForward();
      expect(navigation.activeView).toBe('settings');
      expect(navigation.settingsSection).toBe('doctor');
    });

    it('keeps the entries either side when the section changes', async () => {
      const { selectProject, openSettings, goBack, navigation } = await importNavigation();
      openSettings('general');
      selectProject('a');
      await goBack();

      openSettings('tags');

      expect(navigation.history.entries).toEqual([
        [home],
        [home, settings('tags')],
        [projectRoute('a')],
      ]);
      expect(navigation.canGoBack).toBe(true);
      expect(navigation.canGoForward).toBe(true);
    });

    it('closes with a step back to the view it was opened from', async () => {
      const { selectProject, openSettings, closeSettings, navigation } = await importNavigation();
      selectProject('a');
      openSettings('general');

      closeSettings();

      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      // A step, not a new entry: Forward reopens Settings.
      expect(navigation.history.entries).toHaveLength(3);
      expect(navigation.history.index).toBe(1);
      expect(navigation.canGoForward).toBe(true);
    });

    it('comes back over the view it was opened from when stepped back to', async () => {
      const { selectProject, openSettings, goBack, navigation } = await importNavigation();
      selectProject('a');
      openSettings('general');
      selectProject('b');

      await goBack();

      expect(navigation.detailStack).toEqual([projectRoute('a'), settings('general')]);
      expect(navigation.activeView).toBe('settings');
      // The workspace under it is the project it was opened over, not b.
      expect(navigation.selectedProjectId).toBe('a');
      expect(navigation.projectLayer).toEqual(projectRoute('a'));
    });
  });

  describe('the diff view', () => {
    it('closes with a step back, announced as a pop', async () => {
      const { selectProject, openDiffRoute, closeDiffRouteIfCurrent, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const diffRoute = navigation.currentRoute as DiffDetailRoute;

      closeDiffRouteIfCurrent(diffRoute);

      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      expect(navigation.history.index).toBe(1);
      expect(navigation.canGoForward).toBe(true);
      expect(dispatchEvent).toHaveBeenCalledTimes(1);
      const event = dispatchEvent.mock.calls[0][0] as CustomEvent;
      expect(event.type).toBe('staged:diff-route-popped');
      expect(event.detail.route).toBe(diffRoute);
    });

    it('ignores a close for a diff that is no longer the view on screen', async () => {
      const { selectProject, openDiffRoute, closeDiffRouteIfCurrent, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const diffRoute = navigation.currentRoute as DiffDetailRoute;
      selectProject('b');

      closeDiffRouteIfCurrent(diffRoute);

      expect(navigation.currentRoute).toEqual(projectRoute('b'));
      expect(navigation.history.index).toBe(3);
    });

    it('opens over its own project from another view, and closes onto that project', async () => {
      const { selectProject, openDiffRoute, closeDiffRouteIfCurrent, goBack, navigation } =
        await importNavigation();
      selectProject('a');

      openDiffRoute({ branchId: 'b9', projectId: 'b' });
      const diffRoute = navigation.currentRoute as DiffDetailRoute;

      // One entry for the diff over its project: b itself was not visited.
      expect(navigation.detailStack).toEqual([projectRoute('b'), diffRoute]);
      expect(navigation.history.entries).toHaveLength(3);
      expect(navigation.selectedProjectId).toBe('b');
      expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', 'b');

      // Closing shows what was under the diff. The user didn't come from
      // there, so it is a visit, and Back from it returns to the diff.
      closeDiffRouteIfCurrent(diffRoute);
      expect(navigation.detailStack).toEqual([projectRoute('b')]);
      expect(navigation.history.entries).toHaveLength(4);
      expect(dispatchEvent.mock.calls.map(([event]) => event.type)).toEqual([
        'staged:diff-route-popped',
      ]);

      // The diff as the backend has it now: an equal route, not the same one.
      await goBack();
      expect(navigation.currentRoute).toEqual(diffRoute);
      await goBack();
      expect(navigation.currentRoute).toEqual(projectRoute('a'));
    });

    it('takes the place of the diff on screen without a new entry or an announcement', async () => {
      const { selectProject, openDiffRoute, navigation } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });

      openDiffRoute({ branchId: 'b1', projectId: 'a', commitSha: 'abc' });

      expect(navigation.history.entries).toHaveLength(3);
      expect(navigation.currentRoute).toMatchObject({ kind: 'diff', commitSha: 'abc' });
      expect(dispatchEvent).not.toHaveBeenCalled();
    });

    it('waits for the comment being typed to save before a step leaves it', async () => {
      const { selectProject, openDiffRoute, registerDiffLeaveCheck, goBack, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const diffRoute = navigation.currentRoute;
      let finishSave!: (saved: boolean) => void;
      const check = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
      registerDiffLeaveCheck(check);

      const stepping = goBack();
      await vi.waitFor(() => expect(check).toHaveBeenCalledTimes(1));
      // Still on the diff while the save runs.
      expect(navigation.currentRoute).toBe(diffRoute);

      finishSave(true);
      await expect(stepping).resolves.toBe(true);

      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      // What was under the diff: a reference that led to it can come back.
      expect(dispatchEvent.mock.calls.map(([event]) => event.type)).toEqual([
        'staged:diff-route-popped',
      ]);
    });

    it('stays on the diff when the comment could not be saved, or the save throws', async () => {
      const error = vi.spyOn(console, 'error').mockImplementation(() => {});
      const { selectProject, openDiffRoute, registerDiffLeaveCheck, goBack, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const diffRoute = navigation.currentRoute;

      registerDiffLeaveCheck(vi.fn().mockResolvedValue(false));
      await expect(goBack()).resolves.toBe(false);
      expect(navigation.currentRoute).toBe(diffRoute);

      registerDiffLeaveCheck(vi.fn().mockRejectedValue(new Error('save failed')));
      await expect(goBack()).resolves.toBe(false);
      expect(navigation.currentRoute).toBe(diffRoute);
      expect(error).toHaveBeenCalledTimes(1);
      expect(dispatchEvent).not.toHaveBeenCalled();
    });

    it('does not save for a step with nowhere to go', async () => {
      const { selectProject, openDiffRoute, registerDiffLeaveCheck, goForward } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const check = vi.fn().mockResolvedValue(true);
      registerDiffLeaveCheck(check);

      await expect(goForward()).resolves.toBe(false);

      expect(check).not.toHaveBeenCalled();
    });

    it('drops a step when the user navigated, or a dialog opened, during the save', async () => {
      const { selectProject, openDiffRoute, registerDiffLeaveCheck, goBack, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      let finishSave!: (saved: boolean) => void;
      registerDiffLeaveCheck(() => new Promise<boolean>((resolve) => (finishSave = resolve)));

      let stepping = goBack();
      selectProject('b');
      finishSave(true);
      await expect(stepping).resolves.toBe(false);
      expect(navigation.currentRoute).toEqual(projectRoute('b'));

      openDiffRoute({ branchId: 'b2', projectId: 'b' });
      const diffRoute = navigation.currentRoute;
      stepping = goBack();
      dialogs.push({ getAttribute: () => 'open' });
      finishSave(true);
      await expect(stepping).resolves.toBe(false);
      expect(navigation.currentRoute).toBe(diffRoute);
    });

    it('announces a diff as replaced when a step lands somewhere other than under it', async () => {
      const { selectProject, openDiffRoute, goBack, goForward, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const diffRoute = navigation.currentRoute;
      selectProject('b');
      dispatchEvent.mockClear();

      // b → the diff → b again: forward from the diff lands on b, which is
      // not what was under it.
      await goBack();
      const stepped = navigation.currentRoute;
      expect(stepped).toEqual(diffRoute);
      await goForward();

      expect(dispatchEvent).toHaveBeenCalledTimes(1);
      const event = dispatchEvent.mock.calls[0][0] as CustomEvent;
      expect(event.type).toBe('staged:diff-route-replaced');
      expect(event.detail.route).toBe(stepped);
    });
  });

  // A diff route is whatever its opener had on screen. Returned to through
  // the history it may be long out of date, or of something deleted, so a
  // step asks about the diff it is about to land on (diffRouteRefresh.ts,
  // replaced here by `refreshDiffRoute`).
  describe('stepping onto a diff entry', () => {
    const commit = (sha: string) => ({ sha, subject: `commit ${sha}` });
    const eventTypes = () => dispatchEvent.mock.calls.map(([event]) => event.type);

    it('steps Back onto the diff, brought up to date with its branch', async () => {
      const { selectProject, openDiffRoute, goBack, navigation } = await importNavigation();
      selectProject('a');
      openDiffRoute({
        branchId: 'b1',
        projectId: 'a',
        commits: [commit('old')] as never,
        branchLabel: 'feature',
      });
      const opened = navigation.currentRoute as DiffDetailRoute;
      selectProject('b');
      dispatchEvent.mockClear();
      setStoreValue.mockClear();
      refreshDiffRoute.mockImplementationOnce(async (route: DiffDetailRoute) => ({
        status: 'current',
        route: { ...route, commits: [commit('old'), commit('new')], branchLabel: 'renamed' },
      }));

      await expect(goBack()).resolves.toBe(true);

      // Asked about the entry as it was opened, and showing what came back.
      expect(refreshDiffRoute).toHaveBeenCalledTimes(1);
      expect(refreshDiffRoute.mock.calls[0][0]).toBe(opened);
      const shown = navigation.currentRoute as DiffDetailRoute;
      expect(shown.kind).toBe('diff');
      expect(shown.branchId).toBe('b1');
      expect(shown.commits).toEqual([commit('old'), commit('new')]);
      expect(shown.branchLabel).toBe('renamed');
      // Over its project, which is the window's project again.
      expect(navigation.detailStack).toEqual([projectRoute('a'), shown]);
      expect(navigation.workspaceParked).toBe(true);
      expect(navigation.selectedProjectId).toBe('a');
      expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', 'a');
      // The history is the same three views, the diff entry updated in place.
      expect(navigation.history.entries).toHaveLength(4);
      expect(navigation.history.index).toBe(2);
      expect(navigation.canGoForward).toBe(true);
      // Arriving at a diff announces nothing: no diff left the screen.
      expect(dispatchEvent).not.toHaveBeenCalled();
    });

    it('steps Forward onto the diff, brought up to date with its branch', async () => {
      const { selectProject, openDiffRoute, goBack, goForward, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a', commits: [commit('old')] as never });
      await goBack();
      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      // Stepping Back to the project needed no answer about any diff.
      expect(refreshDiffRoute).not.toHaveBeenCalled();
      dispatchEvent.mockClear();
      refreshDiffRoute.mockImplementationOnce(async (route: DiffDetailRoute) => ({
        status: 'current',
        route: { ...route, commits: [commit('rebased')] },
      }));

      await expect(goForward()).resolves.toBe(true);

      const shown = navigation.currentRoute as DiffDetailRoute;
      expect(shown.kind).toBe('diff');
      expect(shown.commits).toEqual([commit('rebased')]);
      expect(navigation.detailStack).toEqual([projectRoute('a'), shown]);
      expect(navigation.canGoForward).toBe(false);
      expect(dispatchEvent).not.toHaveBeenCalled();
    });

    it('leaves the diff it stepped onto as any diff is left', async () => {
      const {
        selectProject,
        openDiffRoute,
        closeDiffRouteIfCurrent,
        goBack,
        goForward,
        navigation,
      } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      selectProject('b');
      refreshDiffRoute.mockImplementation(async (route: DiffDetailRoute) => ({
        status: 'current',
        route: { ...route },
      }));

      // Back onto the diff, then Back again to the project under it: a pop,
      // so a reference that led to the diff can come back.
      await goBack();
      const first = navigation.currentRoute as DiffDetailRoute;
      dispatchEvent.mockClear();
      await goBack();
      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      expect(eventTypes()).toEqual(['staged:diff-route-popped']);
      expect((dispatchEvent.mock.calls[0][0] as CustomEvent).detail.route).toBe(first);

      // Forward onto it again and on to b, which is not what was under it.
      await goForward();
      const second = navigation.currentRoute as DiffDetailRoute;
      dispatchEvent.mockClear();
      await goForward();
      expect(navigation.currentRoute).toEqual(projectRoute('b'));
      expect(eventTypes()).toEqual(['staged:diff-route-replaced']);
      expect((dispatchEvent.mock.calls[0][0] as CustomEvent).detail.route).toBe(second);

      // Its close button works on the entry that is on screen now.
      await goBack();
      const third = navigation.currentRoute as DiffDetailRoute;
      closeDiffRouteIfCurrent(second);
      expect(navigation.currentRoute).toBe(third);
      closeDiffRouteIfCurrent(third);
      expect(navigation.currentRoute).toEqual(projectRoute('a'));
    });

    // Settings opened from a diff shares the diff's route object with the
    // entry beneath it, and a diff route is the same as another only by
    // identity. These hold the two entries to one route through a refresh.
    describe('with Settings opened over the diff', () => {
      /** a, the b1 diff over a, Settings over that diff, then b. */
      async function diffUnderSettingsThenAway() {
        const nav = await importNavigation();
        nav.selectProject('a');
        nav.openDiffRoute({ branchId: 'b1', projectId: 'a', commits: [commit('old')] as never });
        const opened = nav.navigation.currentRoute as DiffDetailRoute;
        nav.openSettings('general');
        nav.selectProject('b');
        dispatchEvent.mockClear();
        refreshDiffRoute.mockClear();
        return { ...nav, opened };
      }

      it('steps between the diff and Settings over it without asking or announcing', async () => {
        const { selectProject, openDiffRoute, openSettings, closeSettings, navigation, ...nav } =
          await importNavigation();
        selectProject('a');
        openDiffRoute({ branchId: 'b1', projectId: 'a' });
        const diffRoute = navigation.currentRoute as DiffDetailRoute;
        openSettings('general');
        dispatchEvent.mockClear();

        // Back onto the diff beneath: it never left the screen's location.
        await expect(nav.goBack()).resolves.toBe(true);
        expect(navigation.detailStack).toEqual([projectRoute('a'), diffRoute]);
        expect(navigation.currentRoute).toBe(diffRoute);

        // Forward to Settings over it again.
        await expect(nav.goForward()).resolves.toBe(true);
        expect(navigation.detailStack).toEqual([projectRoute('a'), diffRoute, settings('general')]);
        expect(navigation.detailStack[1]).toBe(diffRoute);

        expect(refreshDiffRoute).not.toHaveBeenCalled();
        expect(dispatchEvent).not.toHaveBeenCalled();

        // Closing Settings steps back onto the diff: no new entry, and
        // Forward reopens Settings.
        closeSettings();
        expect(navigation.currentRoute).toBe(diffRoute);
        expect(navigation.history.entries).toHaveLength(4);
        expect(navigation.history.index).toBe(2);
        expect(navigation.canGoForward).toBe(true);
        expect(dispatchEvent).not.toHaveBeenCalled();
      });

      it("takes those steps in the caller's tick", async () => {
        const { selectProject, openDiffRoute, openSettings, goBack, goForward, navigation } =
          await importNavigation();
        selectProject('a');
        openDiffRoute({ branchId: 'b1', projectId: 'a' });
        const diffRoute = navigation.currentRoute;
        openSettings('general');

        void goBack();
        expect(navigation.currentRoute).toBe(diffRoute);
        void goForward();
        expect(navigation.currentRoute).toEqual(settings('general'));
      });

      it('brings the diff up to date in both entries when stepping onto Settings over it', async () => {
        const { goBack, goForward, closeSettings, navigation, opened } =
          await diffUnderSettingsThenAway();
        refreshDiffRoute.mockImplementationOnce(async (route: DiffDetailRoute) => ({
          status: 'current',
          route: { ...route, commits: [commit('old'), commit('new')] },
        }));

        // Back from b onto Settings over the diff: the diff is not on screen,
        // so it is asked about, once.
        await expect(goBack()).resolves.toBe(true);
        expect(refreshDiffRoute).toHaveBeenCalledTimes(1);
        expect(refreshDiffRoute.mock.calls[0][0]).toBe(opened);
        const refreshed = navigation.detailStack[1] as DiffDetailRoute;
        expect(refreshed).not.toBe(opened);
        expect(refreshed.commits).toEqual([commit('old'), commit('new')]);
        expect(navigation.currentRoute).toEqual(settings('general'));
        // The entry beneath holds the very same route, not a second copy.
        const [, , beneath, over] = navigation.history.entries;
        expect(over).toBe(navigation.detailStack);
        expect(beneath).toHaveLength(2);
        expect(beneath[1]).toBe(refreshed);
        expect(navigation.history.entries).toHaveLength(5);
        expect(navigation.history.index).toBe(3);
        // The swap itself announced nothing.
        expect(dispatchEvent).not.toHaveBeenCalled();

        // Back onto the diff beneath, and Forward to Settings again: the
        // diff is on screen throughout, so nothing is asked or announced.
        await expect(goBack()).resolves.toBe(true);
        expect(navigation.currentRoute).toBe(refreshed);
        await expect(goForward()).resolves.toBe(true);
        expect(navigation.currentRoute).toEqual(settings('general'));
        expect(navigation.detailStack[1]).toBe(refreshed);
        expect(refreshDiffRoute).toHaveBeenCalledTimes(1);
        expect(dispatchEvent).not.toHaveBeenCalled();

        // Closing Settings steps back rather than adding an entry.
        closeSettings();
        expect(navigation.currentRoute).toBe(refreshed);
        expect(navigation.history.entries).toHaveLength(5);
        expect(navigation.history.index).toBe(2);
        expect(navigation.canGoForward).toBe(true);
        expect(dispatchEvent).not.toHaveBeenCalled();
        // And the next Back is the project under the diff, not Settings.
        await expect(goBack()).resolves.toBe(true);
        expect(navigation.detailStack).toEqual([projectRoute('a')]);
        expect(eventTypes()).toEqual(['staged:diff-route-popped']);
      });

      it('brings the diff up to date in both entries when stepping onto the diff itself', async () => {
        const { goBack, goForward, closeSettings, navigation, opened } =
          await diffUnderSettingsThenAway();
        // From b, Back three times: Settings, the diff beneath, then a.
        await goBack();
        await goBack();
        await goBack();
        expect(navigation.detailStack).toEqual([projectRoute('a')]);
        const afterFirst = navigation.history.entries[2][1] as DiffDetailRoute;
        expect(afterFirst).not.toBe(opened);
        dispatchEvent.mockClear();
        refreshDiffRoute.mockClear();

        // Forward from a onto the diff: asked about again, and the Settings
        // entry ahead of it follows.
        await expect(goForward()).resolves.toBe(true);
        expect(refreshDiffRoute).toHaveBeenCalledTimes(1);
        expect(refreshDiffRoute.mock.calls[0][0]).toBe(afterFirst);
        const shown = navigation.currentRoute as DiffDetailRoute;
        expect(shown).not.toBe(afterFirst);
        expect(navigation.history.entries[3][1]).toBe(shown);
        expect(dispatchEvent).not.toHaveBeenCalled();

        await expect(goForward()).resolves.toBe(true);
        expect(navigation.currentRoute).toEqual(settings('general'));
        expect(refreshDiffRoute).toHaveBeenCalledTimes(1);
        expect(dispatchEvent).not.toHaveBeenCalled();
        closeSettings();
        expect(navigation.currentRoute).toBe(shown);
        expect(navigation.history.index).toBe(2);
        expect(dispatchEvent).not.toHaveBeenCalled();
      });
    });

    it('steps over a diff whose branch is gone, and drops every entry for the branch', async () => {
      const { selectProject, openDiffRoute, goBack, navigation } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      selectProject('b');
      openDiffRoute({ branchId: 'b1', projectId: 'a', commitSha: 'abc' });
      selectProject('c');
      openDiffRoute({ branchId: 'b2', projectId: 'c' });
      const kept = navigation.currentRoute;
      selectProject('d');
      refreshDiffRoute.mockImplementation(async (route: DiffDetailRoute) =>
        route.branchId === 'b1' ? { status: 'gone', what: 'branch' } : { status: 'current', route }
      );
      dispatchEvent.mockClear();

      // d → the b2 diff, whose branch is still there.
      await expect(goBack()).resolves.toBe(true);
      expect(navigation.currentRoute).toBe(kept);
      // → c, then the step that would land on the second b1 diff lands on b.
      await goBack();
      await expect(goBack()).resolves.toBe(true);
      expect(navigation.currentRoute).toEqual(projectRoute('b'));
      // Both b1 entries went on the one answer; the diff view never mounted.
      expect(navigation.history.entries).toEqual([
        [home],
        [projectRoute('a')],
        [projectRoute('b')],
        [projectRoute('c')],
        [projectRoute('c'), kept],
        [projectRoute('d')],
      ]);
      expect(refreshDiffRoute.mock.calls.filter(([route]) => route.branchId === 'b1')).toHaveLength(
        1
      );
      await goBack();
      expect(navigation.currentRoute).toEqual(projectRoute('a'));
    });

    it('steps over a diff whose review is gone, keeping the other diffs of its branch', async () => {
      const { selectProject, openDiffRoute, goBack, navigation } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const branchDiff = navigation.currentRoute;
      selectProject('b');
      openDiffRoute({ branchId: 'b1', projectId: 'a', reviewId: 'r1' });
      selectProject('c');
      refreshDiffRoute.mockImplementation(async (route: DiffDetailRoute) =>
        route.reviewId === 'r1' ? { status: 'gone', what: 'review' } : { status: 'current', route }
      );

      await expect(goBack()).resolves.toBe(true);

      expect(navigation.currentRoute).toEqual(projectRoute('b'));
      expect(navigation.history.entries).toEqual([
        [home],
        [projectRoute('a')],
        [projectRoute('a'), branchDiff],
        [projectRoute('b')],
        [projectRoute('c')],
      ]);
      await goBack();
      expect(navigation.currentRoute).toBe(branchDiff);
    });

    it('goes nowhere when the diff that is gone was the only entry that way', async () => {
      const { selectProject, openDiffRoute, goBack, goForward, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      await goBack();
      expect(navigation.canGoForward).toBe(true);
      refreshDiffRoute.mockResolvedValue({ status: 'gone', what: 'branch' });
      setStoreValue.mockClear();

      await expect(goForward()).resolves.toBe(false);

      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      expect(navigation.canGoForward).toBe(false);
      expect(navigation.history.entries).toEqual([[home], [projectRoute('a')]]);
      expect(setStoreValue).not.toHaveBeenCalled();
    });

    it('asks about each diff in turn until one can be shown', async () => {
      const { selectProject, openDiffRoute, goBack, navigation } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const kept = navigation.currentRoute;
      selectProject('b');
      openDiffRoute({ branchId: 'b2', projectId: 'b' });
      selectProject('c');
      openDiffRoute({ branchId: 'b3', projectId: 'c' });
      selectProject('d');
      refreshDiffRoute.mockImplementation(async (route: DiffDetailRoute) =>
        route.branchId === 'b3' ? { status: 'gone', what: 'branch' } : { status: 'current', route }
      );

      // d → (the b3 diff, gone) → c.
      await goBack();
      expect(navigation.currentRoute).toEqual(projectRoute('c'));
      refreshDiffRoute.mockImplementation(async (route: DiffDetailRoute) =>
        route.branchId === 'b2' ? { status: 'gone', what: 'branch' } : { status: 'current', route }
      );
      // c → (the b2 diff, gone) → b, and on to the b1 diff, which is fine.
      await goBack();
      expect(navigation.currentRoute).toEqual(projectRoute('b'));
      await goBack();
      expect(navigation.currentRoute).toBe(kept);
    });

    it('leaves the diff on screen in place when another entry for its branch is dropped', async () => {
      const { selectProject, openDiffRoute, forgetProject, goBack, goForward, navigation } =
        await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      selectProject('b');
      openDiffRoute({ branchId: 'b1', projectId: 'a', commitSha: 'abc' });
      const onScreen = navigation.currentRoute;
      // With b gone the two b1 diffs are neighbours: Back from one is the other.
      forgetProject('b');
      expect(navigation.history.entries).toHaveLength(4);
      refreshDiffRoute.mockResolvedValue({ status: 'gone', what: 'branch' });
      dispatchEvent.mockClear();

      await expect(goBack()).resolves.toBe(true);

      // The earlier diff went and the step landed beyond it. The diff the
      // user was on is the diff view's to close, so its entry stayed and the
      // step left it as any step does: announced, here as a pop.
      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      expect(navigation.history.entries).toEqual([
        [home],
        [projectRoute('a')],
        [projectRoute('a'), onScreen],
      ]);
      expect(eventTypes()).toEqual(['staged:diff-route-popped']);

      // Stepping onto it again is what finds it gone.
      await expect(goForward()).resolves.toBe(false);
      expect(navigation.history.entries).toEqual([[home], [projectRoute('a')]]);
    });

    it('shows the diff as it was opened when the backend does not say', async () => {
      const { selectProject, openDiffRoute, goBack, navigation } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const opened = navigation.currentRoute;
      selectProject('b');
      refreshDiffRoute.mockResolvedValueOnce({ status: 'unknown' });

      await expect(goBack()).resolves.toBe(true);

      expect(navigation.currentRoute).toBe(opened);
    });

    it('drops the step when the user navigated, or a dialog opened, while it asked', async () => {
      const { selectProject, openDiffRoute, goBack, navigation } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      selectProject('b');
      let answer!: (result: unknown) => void;
      refreshDiffRoute.mockImplementation(
        (route: DiffDetailRoute) =>
          new Promise((resolve) => (answer = () => resolve({ status: 'current', route })))
      );

      let stepping = goBack();
      // Still on b while the backend is asked.
      expect(navigation.currentRoute).toEqual(projectRoute('b'));
      selectProject('c');
      answer(null);
      await expect(stepping).resolves.toBe(false);
      expect(navigation.currentRoute).toEqual(projectRoute('c'));

      // c → b needs no answer; b → the diff does.
      await goBack();
      stepping = goBack();
      dialogs.push({ getAttribute: () => 'open' });
      answer(null);
      await expect(stepping).resolves.toBe(false);
      expect(navigation.currentRoute).toEqual(projectRoute('b'));
    });

    it('does not save the comment on the diff being left when the step turns out to go nowhere', async () => {
      const {
        selectProject,
        openDiffRoute,
        forgetProject,
        registerDiffLeaveCheck,
        goBack,
        goForward,
        navigation,
      } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const first = navigation.currentRoute;
      selectProject('b');
      openDiffRoute({ branchId: 'b2', projectId: 'a' });
      forgetProject('b');
      const check = vi.fn().mockResolvedValue(true);
      registerDiffLeaveCheck(check);

      // Back from the b2 diff onto the b1 diff: somewhere to go, so it saves.
      await expect(goBack()).resolves.toBe(true);
      const stepped = navigation.currentRoute;
      expect(stepped).toEqual(first);
      expect(check).toHaveBeenCalledTimes(1);
      check.mockClear();

      // Forward again, but the b2 branch has gone meanwhile. Saving clears
      // the editor, so a step that goes nowhere must not have asked for it.
      refreshDiffRoute.mockResolvedValue({ status: 'gone', what: 'branch' });
      await expect(goForward()).resolves.toBe(false);

      expect(check).not.toHaveBeenCalled();
      expect(navigation.currentRoute).toBe(stepped);
      expect(navigation.canGoForward).toBe(false);
    });
  });

  describe('boards that are no longer offered', () => {
    const board = (groupId: string) => ({ kind: 'board', groupId });
    /** The catalog loads with these boards pinned; any other is not offered. */
    const pin = (...groupIds: string[]) => {
      tagsStoreMock.loaded = true;
      tagsStoreMock.catalog.groupsById = new Map(
        groupIds.map((id) => [id, { showInSidebar: true }])
      );
    };

    it('steps Back over a board that is in the history but neither on screen nor retained', async () => {
      const { restoreInitialRoute, selectBoard, selectProject, goBack, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectBoard('g1');
      selectBoard('g2');
      selectProject('a');
      // Only the project is mounted: g1 has no view to notice it is gone.
      expect(navigation.projectLayer).toEqual(projectRoute('a'));
      pin('g2');
      const mounted: unknown[] = [];

      await expect(goBack()).resolves.toBe(true);
      mounted.push(navigation.projectLayer);
      expect(navigation.currentRoute).toEqual(board('g2'));
      // g1's entry went before the step, not on arrival at it.
      expect(navigation.history.entries).toEqual([[home], [board('g2')], [projectRoute('a')]]);

      await expect(goBack()).resolves.toBe(true);
      mounted.push(navigation.projectLayer);
      // One step from g2 to the view before the dead board.
      expect(navigation.currentRoute).toEqual(home);
      expect(navigation.canGoBack).toBe(false);
      // The dead board was never the layer's view, so it never mounted.
      expect(mounted).toEqual([board('g2'), board('g2')]);
    });

    it('steps Forward over a board that is neither on screen nor retained', async () => {
      const { restoreInitialRoute, selectBoard, selectProject, goBack, goForward, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectProject('a');
      selectBoard('g1');
      selectProject('b');
      await goBack();
      await goBack();
      await goBack();
      // At the grid, with project a retained under it and g1 two steps ahead.
      expect(navigation.currentRoute).toEqual(home);
      await goForward();
      expect(navigation.projectLayer).toEqual(projectRoute('a'));
      pin();

      await expect(goForward()).resolves.toBe(true);

      expect(navigation.currentRoute).toEqual(projectRoute('b'));
      expect(navigation.projectLayer).toEqual(projectRoute('b'));
      expect(navigation.history.entries).toEqual([
        [home],
        [projectRoute('a')],
        [projectRoute('b')],
      ]);
      expect(navigation.canGoForward).toBe(false);
    });

    it('goes nowhere when the dead board was the only entry that way', async () => {
      const { restoreInitialRoute, selectBoard, goBack, goForward, selectProject, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectProject('a');
      selectBoard('g1');
      await goBack();
      // On a with the board ahead. The board's layer was replaced by a's.
      expect(navigation.projectLayer).toEqual(projectRoute('a'));
      pin();

      await expect(goForward()).resolves.toBe(false);

      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      expect(navigation.canGoForward).toBe(false);
      expect(navigation.history.entries).toEqual([[home], [projectRoute('a')]]);
    });

    it('keeps what was open over the board, over the grid instead', async () => {
      const { restoreInitialRoute, selectBoard, openSettings, selectProject, goBack, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectBoard('g1');
      openSettings('tags');
      selectProject('a');
      pin();

      await expect(goBack()).resolves.toBe(true);

      expect(navigation.detailStack).toEqual([home, settings('tags')]);
      expect(navigation.history.entries).toEqual([
        [home],
        [home, settings('tags')],
        [projectRoute('a')],
      ]);
    });

    it('keeps every board while the tag catalog has not loaded', async () => {
      const { restoreInitialRoute, selectBoard, selectProject, goBack, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectBoard('g1');
      selectProject('a');
      tagsStoreMock.loaded = false;

      await goBack();

      expect(navigation.currentRoute).toEqual(board('g1'));
    });

    it('keeps a board that is still pinned', async () => {
      const { restoreInitialRoute, selectBoard, selectProject, goBack, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectBoard('g1');
      selectProject('a');
      pin('g1');

      await goBack();

      expect(navigation.currentRoute).toEqual(board('g1'));
    });

    it('drops a board whose group is still there but no longer pinned', async () => {
      const { restoreInitialRoute, selectBoard, selectProject, goBack, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectBoard('g1');
      selectProject('a');
      tagsStoreMock.loaded = true;
      tagsStoreMock.catalog.groupsById = new Map([['g1', { showInSidebar: false }]]);

      await goBack();

      expect(navigation.currentRoute).toEqual(home);
    });
  });

  describe('projects that are gone', () => {
    it('forgets a project wherever it sits in the history', async () => {
      const { selectProject, openDiffRoute, openSettings, forgetProject, goBack, navigation } =
        await importNavigation();
      selectProject('a');
      openSettings('general');
      selectProject('b');
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      selectProject('c');

      forgetProject('a');

      // Its own entries, Settings over it and the diff of its branch all go.
      expect(navigation.history.entries).toEqual([
        [home],
        [projectRoute('b')],
        [projectRoute('c')],
      ]);
      expect(navigation.currentRoute).toEqual(projectRoute('c'));

      await goBack();
      expect(navigation.currentRoute).toEqual(projectRoute('b'));
    });

    it('merges the entries a forgotten project sat between', async () => {
      const { selectProject, goHome, forgetProject, navigation } = await importNavigation();
      selectProject('a');
      goHome();

      forgetProject('a');

      expect(navigation.history.entries).toEqual([[home]]);
      expect(navigation.canGoBack).toBe(false);
    });

    it('lands on the nearest earlier entry when the project on screen is forgotten', async () => {
      const { selectProject, forgetProject, navigation } = await importNavigation();
      selectProject('a');
      selectProject('b');

      forgetProject('b');

      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      expect(navigation.canGoForward).toBe(false);
      expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', 'a');
    });

    it('lands on the grid when nothing earlier is left', async () => {
      snapshotProjectId = 'a';
      const { forgetProject, navigation } = await importNavigation();

      forgetProject('a');

      expect(navigation.history.entries).toEqual([[home]]);
      expect(navigation.homeActive).toBe(true);
      expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', null);
    });

    // What a mounted project view does when its project disappears from the
    // list (ProjectHome's effect): a delete in another window, or anything
    // else that didn't navigate away first.
    it('leaves a project that disappeared while on screen for the grid, and forgets it', async () => {
      const {
        restoreInitialRoute,
        selectProject,
        leaveGoneProject,
        goBack,
        goForward,
        navigation,
      } = await importNavigation();
      await restoreInitialRoute();
      selectProject('a');
      selectProject('b');
      selectProject('a');

      leaveGoneProject('a', true);

      // On the grid, and a is nowhere in the history: Back goes to b.
      expect(navigation.currentRoute).toEqual(home);
      expect(navigation.homeActive).toBe(true);
      expect(navigation.history.entries).toEqual([[home], [projectRoute('b')], [home]]);
      expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', null);
      await goBack();
      expect(navigation.currentRoute).toEqual(projectRoute('b'));
      await goBack();
      expect(navigation.currentRoute).toEqual(home);
      await expect(goBack()).resolves.toBe(false);
      await goForward();
      await goForward();
      await expect(goForward()).resolves.toBe(false);
      expect(navigation.currentRoute).toEqual(home);
    });

    it('leaves the grid alone in the history when the project was opened from it', async () => {
      const { restoreInitialRoute, selectProject, leaveGoneProject, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectProject('a');

      leaveGoneProject('a', true);

      // home → a → home, minus a: one grid entry, not two to step between.
      expect(navigation.history.entries).toEqual([[home]]);
      expect(navigation.canGoBack).toBe(false);
      expect(navigation.canGoForward).toBe(false);
    });

    it('releases a project that disappeared while retained under the grid, without moving', async () => {
      const { restoreInitialRoute, selectProject, goHome, leaveGoneProject, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectProject('b');
      selectProject('a');
      goHome();
      expect(navigation.projectLayer).toEqual(projectRoute('a'));
      setStoreValue.mockClear();

      leaveGoneProject('a', false);

      // The view on screen did not change, the layer unmounts, and Back from
      // the grid is b rather than the project that is gone.
      expect(navigation.currentRoute).toEqual(home);
      expect(navigation.projectLayer).toBeNull();
      expect(navigation.history.entries).toEqual([[home], [projectRoute('b')], [home]]);
      expect(navigation.history.index).toBe(2);
      expect(setStoreValue).not.toHaveBeenCalled();
    });

    it('leaves a project that disappeared from under Settings or a diff', async () => {
      const { restoreInitialRoute, selectProject, openSettings, leaveGoneProject, navigation } =
        await importNavigation();
      await restoreInitialRoute();
      selectProject('a');
      openSettings('general');

      // The project view is parked under Settings, but it is still the
      // workspace's view: its layer is the active one.
      leaveGoneProject('a', true);

      expect(navigation.detailStack).toEqual([home]);
      expect(navigation.history.entries).toEqual([[home]]);
    });

    it('steps over a project deleted in another window', async () => {
      const { selectProject, goBack, navigation } = await importNavigation();
      selectProject('a');
      selectProject('b');
      selectProject('c');
      // The list this window loaded no longer has b.
      projectsStoreMock.loaded = true;
      projectsStoreMock.projects = [{ id: 'a' }, { id: 'c' }];

      await expect(goBack()).resolves.toBe(true);

      expect(navigation.currentRoute).toEqual(projectRoute('a'));
      expect(navigation.history.entries).toEqual([
        [home],
        [projectRoute('a')],
        [projectRoute('c')],
      ]);
    });

    it('steps over a project with a delete in flight', async () => {
      const { selectProject, goBack, navigation } = await importNavigation();
      selectProject('a');
      selectProject('b');
      projectsStoreMock.loaded = true;
      projectsStoreMock.projects = [{ id: 'a' }, { id: 'b' }];
      deletingProjectIds.add('a');

      await goBack();

      expect(navigation.currentRoute).toEqual(home);
    });

    it('keeps every entry while the project list has not loaded', async () => {
      const { selectProject, goBack, navigation } = await importNavigation();
      selectProject('a');
      selectProject('b');
      projectsStoreMock.loaded = false;
      projectsStoreMock.projects = [];

      await goBack();

      expect(navigation.currentRoute).toEqual(projectRoute('a'));
    });
  });
});

describe('next and previous project', () => {
  /** Whether a dialog is open, which is when the shortcuts do nothing. */
  let dialogOpen: boolean;

  beforeEach(() => {
    dialogOpen = false;
    vi.stubGlobal('document', {
      querySelectorAll: () => [],
      querySelector: () => (dialogOpen ? {} : null),
    });
    // Listed newest first as [a, b, c] and sorted, by the window's sort, as
    // [c, a, b]: the order the sidebar shows, and the one to walk.
    projectsStoreMock.projects = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];
    projectOrderMock.sortedProjects = [{ id: 'c' }, { id: 'a' }, { id: 'b' }];
  });

  it('walk the sorted list, not the order the projects are listed in', async () => {
    const { selectProject, selectNextProject, selectPreviousProject, navigation } =
      await importNavigation();
    selectProject('a');

    selectNextProject();
    expect(navigation.selectedProjectId).toBe('b');

    selectPreviousProject();
    selectPreviousProject();
    expect(navigation.selectedProjectId).toBe('c');
  });

  it('stop at the ends of the sorted list', async () => {
    const { selectProject, selectNextProject, selectPreviousProject, navigation } =
      await importNavigation();
    selectProject('c');
    selectPreviousProject();
    expect(navigation.selectedProjectId).toBe('c');

    selectProject('b');
    selectNextProject();
    expect(navigation.selectedProjectId).toBe('b');
  });

  it('follow the list when the sort changes', async () => {
    const { selectProject, selectNextProject, navigation } = await importNavigation();
    selectProject('a');
    projectOrderMock.sortedProjects = [{ id: 'b' }, { id: 'a' }, { id: 'c' }];

    selectNextProject();

    expect(navigation.selectedProjectId).toBe('c');
  });

  it('skip a project being deleted, which the sorted list no longer holds', async () => {
    const { selectProject, selectNextProject, navigation } = await importNavigation();
    selectProject('c');
    projectOrderMock.sortedProjects = [{ id: 'c' }, { id: 'b' }];

    selectNextProject();

    expect(navigation.selectedProjectId).toBe('b');
  });

  it('do nothing off a project, or under a dialog', async () => {
    const { selectProject, selectNextProject, selectPreviousProject, goHome, navigation } =
      await importNavigation();
    selectNextProject();
    expect(navigation.selectedProjectId).toBeNull();

    selectProject('a');
    dialogOpen = true;
    selectNextProject();
    selectPreviousProject();
    expect(navigation.selectedProjectId).toBe('a');

    dialogOpen = false;
    goHome();
    selectNextProject();
    expect(navigation.currentRoute).toEqual({ kind: 'projects' });
    expect(navigation.selectedProjectId).toBeNull();
  });
});

describe('parked workspace', () => {
  beforeEach(() => {
    vi.stubGlobal('document', { querySelectorAll: () => [] });
  });

  it('keeps the repos list as the workspace under settings and restores it on Back', async () => {
    const { showAllRepos, openSettings, goBack, navigation } = await importNavigation();

    showAllRepos();
    expect(navigation.showReposList).toBe(true);
    expect(navigation.workspaceParked).toBe(false);

    openSettings('general');
    expect(navigation.activeView).toBe('settings');
    expect(navigation.workspaceParked).toBe(true);
    // The workspace's inputs come from the route underneath, so ReposListView
    // stays mounted rather than being swapped for ProjectsList under the cover.
    expect(navigation.showReposList).toBe(true);
    expect(navigation.selectedProjectId).toBeNull();

    await goBack();
    expect(navigation.workspaceParked).toBe(false);
    expect(navigation.showReposList).toBe(true);
  });

  it('keeps the project under a diff and settings and parks for both', async () => {
    // Leaving the diff route announces itself on `window` (node has none).
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const { selectProject, openDiffRoute, openSettings, goBack, navigation } =
      await importNavigation();

    selectProject('p1');
    openDiffRoute({ branchId: 'b1', projectId: 'p1' });
    expect(navigation.workspaceParked).toBe(true);
    expect(navigation.activeView).toBe('workspace');
    expect(navigation.selectedProjectId).toBe('p1');

    openSettings('doctor');
    expect(navigation.workspaceParked).toBe(true);
    expect(navigation.activeView).toBe('settings');
    expect(navigation.settingsSection).toBe('doctor');
    expect(navigation.selectedProjectId).toBe('p1');

    await goBack();
    expect(navigation.currentRoute.kind).toBe('diff');
    expect(navigation.workspaceParked).toBe(true);

    await goBack();
    expect(navigation.currentRoute).toEqual({ kind: 'project', projectId: 'p1' });
    expect(navigation.workspaceParked).toBe(false);
  });
});

// A project opened from over the workspace (the ⌘K palette is how) takes the
// place of Settings or the diff view rather than opening under it, and Back
// returns to them.
describe('selectProject from Settings and the diff view', () => {
  const home = { kind: 'projects' };
  const projectRoute = (projectId: string) => ({ kind: 'project', projectId });

  beforeEach(() => {
    vi.stubGlobal('document', { querySelectorAll: () => [] });
  });

  it('leaves Settings for the project, and steps back to Settings where it was', async () => {
    const { selectProject, openSettings, goBack, navigation } = await importNavigation();
    selectProject('a');
    openSettings('general');

    selectProject('b');

    expect(navigation.detailStack).toEqual([projectRoute('b')]);
    expect(navigation.activeView).toBe('workspace');
    expect(navigation.workspaceParked).toBe(false);
    expect(navigation.selectedProjectId).toBe('b');
    expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', 'b');

    await goBack();
    expect(navigation.detailStack).toEqual([
      projectRoute('a'),
      { kind: 'settings', section: 'general' },
    ]);
  });

  it('closes Settings onto the project underneath when that project is picked', async () => {
    const { selectProject, openSettings, navigation } = await importNavigation();
    selectProject('a');
    openSettings('keyboard');

    selectProject('a');

    expect(navigation.detailStack).toEqual([projectRoute('a')]);
    expect(navigation.activeView).toBe('workspace');
  });

  it('opens a project from Settings over home', async () => {
    const { selectProject, openSettings, navigation } = await importNavigation();
    openSettings('general');

    selectProject('a');

    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  it('leaves the repos list under Settings for the project', async () => {
    const { selectProject, openSettings, showAllRepos, navigation } = await importNavigation();
    showAllRepos();
    openSettings('general');

    selectProject('b');

    expect(navigation.detailStack).toEqual([projectRoute('b')]);
    expect(navigation.showReposList).toBe(false);
  });

  it('leaves a diff, and Settings over a diff, for the project', async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal('window', { dispatchEvent });
    const { selectProject, openDiffRoute, openSettings, navigation } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });

    selectProject('b');
    expect(navigation.detailStack).toEqual([projectRoute('b')]);
    expect(navigation.workspaceParked).toBe(false);

    openDiffRoute({ branchId: 'b2', projectId: 'b' });
    openSettings('doctor');
    selectProject('c');
    expect(navigation.detailStack).toEqual([projectRoute('c')]);
    expect(navigation.activeView).toBe('workspace');
  });

  it('announces the diff route it replaced, once, and not as a pop', async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal('window', { dispatchEvent });
    const { selectProject, openDiffRoute, navigation } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const diffRoute = navigation.currentRoute;

    selectProject('b');

    expect(dispatchEvent).toHaveBeenCalledTimes(1);
    const event = dispatchEvent.mock.calls[0][0] as CustomEvent;
    expect(event.type).toBe('staged:diff-route-replaced');
    expect(event.detail.route).toBe(diffRoute);

    // An ordinary switch announces nothing.
    selectProject('c');
    expect(dispatchEvent).toHaveBeenCalledTimes(1);
  });

  it('announces a diff as replaced, not popped, by its own project being opened', async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal('window', { dispatchEvent });
    const { selectProject, openDiffRoute } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });

    // The view is now what was under the diff, but the user asked for the
    // project, not to close the diff: a reference that led to it stays shut.
    selectProject('a');

    expect(dispatchEvent.mock.calls.map(([event]) => event.type)).toEqual([
      'staged:diff-route-replaced',
    ]);
  });

  it('is one workspace route whatever it is opened from', async () => {
    const { selectProject, showAllRepos, navigation } = await importNavigation();

    selectProject('a');
    expect(navigation.detailStack).toEqual([projectRoute('a')]);
    selectProject('b');
    expect(navigation.detailStack).toEqual([projectRoute('b')]);
    showAllRepos();
    selectProject('c');
    expect(navigation.detailStack).toEqual([projectRoute('c')]);
    expect(navigation.history.entries).toEqual([
      [home],
      [projectRoute('a')],
      [projectRoute('b')],
      [{ kind: 'repos' }],
      [projectRoute('c')],
    ]);
  });
});

describe('diffLeaveCheck', () => {
  it('is the check the diff view registered, only while the diff is on screen', async () => {
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const {
      selectProject,
      openDiffRoute,
      openSettings,
      closeSettings,
      registerDiffLeaveCheck,
      diffLeaveCheck,
    } = await importNavigation();
    const check = vi.fn().mockResolvedValue(true);
    selectProject('a');
    registerDiffLeaveCheck(check);
    // Nothing to wait for on a project: the caller navigates in the same tick.
    expect(diffLeaveCheck()).toBeNull();

    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    expect(diffLeaveCheck()).toBe(check);
    await expect(diffLeaveCheck()!()).resolves.toBe(true);

    // Settings over the diff: the diff view is not what would be left.
    openSettings('general');
    expect(diffLeaveCheck()).toBeNull();
    closeSettings();
    expect(diffLeaveCheck()).toBe(check);
  });

  it('has nothing to check once the diff view withdraws it', async () => {
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const { selectProject, openDiffRoute, registerDiffLeaveCheck, diffLeaveCheck } =
      await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const unregister = registerDiffLeaveCheck(async () => false);

    unregister();

    expect(diffLeaveCheck()).toBeNull();
  });

  it("keeps a newer diff view's check when an older one withdraws late", async () => {
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const { selectProject, openDiffRoute, registerDiffLeaveCheck, diffLeaveCheck } =
      await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const unregisterOld = registerDiffLeaveCheck(async () => true);
    const newer = async () => true;
    registerDiffLeaveCheck(newer);

    unregisterOld();

    expect(diffLeaveCheck()).toBe(newer);
  });
});

describe('openProjectExternally', () => {
  const home = { kind: 'projects' };
  const projectRoute = (projectId: string) => ({ kind: 'project', projectId });

  /** Stand in for the DOM's dialogs: the helper only asks for the elements
   *  carrying a dialog role and reads their `data-state`. */
  let dialogs: Array<{ getAttribute: (name: string) => string | null }>;
  function addDialog(state: string | null = 'open') {
    dialogs.push({ getAttribute: (name) => (name === 'data-state' ? state : null) });
  }
  let dispatchEvent: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    dialogs = [];
    dispatchEvent = vi.fn();
    vi.stubGlobal('window', { dispatchEvent });
    vi.stubGlobal('document', { querySelectorAll: () => dialogs });
    projectsStoreMock.projects = [{ id: 'a' }, { id: 'b' }];
    projectsStoreMock.loaded = true;
  });

  it('opens a project from home and persists it like any other open', async () => {
    const { openProjectExternally, navigation } = await importNavigation();

    await expect(openProjectExternally('a')).resolves.toBe('opened');

    expect(navigation.detailStack).toEqual([projectRoute('a')]);
    expect(setStoreValue).toHaveBeenCalledWith('last-viewed-project', 'a');
  });

  it('opens over the project on screen, and from Settings over one', async () => {
    const { openProjectExternally, selectProject, openSettings, navigation } =
      await importNavigation();
    selectProject('a');

    await expect(openProjectExternally('b')).resolves.toBe('opened');
    expect(navigation.detailStack).toEqual([projectRoute('b')]);

    openSettings('general');
    await expect(openProjectExternally('a')).resolves.toBe('opened');
    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  it('answers opened for the project already on screen without navigating', async () => {
    const { openProjectExternally, selectProject, navigation } = await importNavigation();
    selectProject('a');
    const stack = navigation.detailStack;
    setStoreValue.mockClear();

    await expect(openProjectExternally('a')).resolves.toBe('opened');

    expect(navigation.detailStack).toBe(stack);
    expect(setStoreValue).not.toHaveBeenCalled();
  });

  it('answers opened for the project already on screen under a dialog', async () => {
    const { openProjectExternally, selectProject, navigation } = await importNavigation();
    selectProject('a');
    addDialog();

    await expect(openProjectExternally('a')).resolves.toBe('opened');

    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  it('refuses while a dialog is open and leaves the route alone', async () => {
    const { openProjectExternally, selectProject, navigation } = await importNavigation();
    selectProject('a');
    addDialog();

    await expect(openProjectExternally('b')).resolves.toBe('dialogOpen');

    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  it('counts a dialog with no data-state, as hand-rolled ones have none', async () => {
    const { openProjectExternally, navigation } = await importNavigation();
    addDialog(null);

    await expect(openProjectExternally('a')).resolves.toBe('dialogOpen');

    expect(navigation.detailStack).toEqual([home]);
  });

  it('ignores a dialog that is only animating out', async () => {
    const { openProjectExternally, navigation } = await importNavigation();
    addDialog('closed');

    await expect(openProjectExternally('a')).resolves.toBe('opened');

    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  it('looks for dialogs and alert dialogs', async () => {
    const querySelectorAll = vi.fn().mockReturnValue([]);
    vi.stubGlobal('document', { querySelectorAll });
    const { openProjectExternally } = await importNavigation();

    await openProjectExternally('a');

    expect(querySelectorAll).toHaveBeenCalledWith('[role="dialog"], [role="alertdialog"]');
  });

  it('leaves the diff view once the comment being typed is saved', async () => {
    const {
      openProjectExternally,
      selectProject,
      openDiffRoute,
      registerDiffLeaveCheck,
      navigation,
    } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const diffRoute = navigation.currentRoute;
    let finishSave!: (saved: boolean) => void;
    const check = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
    registerDiffLeaveCheck(check);

    const opening = openProjectExternally('b');
    await vi.waitFor(() => expect(check).toHaveBeenCalledTimes(1));
    // Still on the diff while the save runs.
    expect(navigation.currentRoute).toBe(diffRoute);

    finishSave(true);
    await expect(opening).resolves.toBe('opened');

    expect(navigation.detailStack).toEqual([projectRoute('b')]);
    expect(dispatchEvent).toHaveBeenCalledTimes(1);
    const event = dispatchEvent.mock.calls[0][0] as CustomEvent;
    expect(event.type).toBe('staged:diff-route-replaced');
    expect(event.detail).toEqual({ route: diffRoute });
  });

  it('leaves a diff over the requested project for the project itself', async () => {
    const {
      openProjectExternally,
      selectProject,
      openDiffRoute,
      registerDiffLeaveCheck,
      navigation,
    } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const check = vi.fn().mockResolvedValue(true);
    registerDiffLeaveCheck(check);

    await expect(openProjectExternally('a')).resolves.toBe('opened');

    expect(check).toHaveBeenCalledTimes(1);
    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  it('stays on the diff when the comment could not be saved', async () => {
    const {
      openProjectExternally,
      selectProject,
      openDiffRoute,
      registerDiffLeaveCheck,
      navigation,
    } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const stack = navigation.detailStack;
    registerDiffLeaveCheck(async () => false);

    await expect(openProjectExternally('b')).resolves.toBe('diffCommentUnsaved');

    expect(navigation.detailStack).toBe(stack);
    expect(dispatchEvent).not.toHaveBeenCalled();
  });

  it('stays on the diff when the save throws', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const {
      openProjectExternally,
      selectProject,
      openDiffRoute,
      registerDiffLeaveCheck,
      navigation,
    } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const stack = navigation.detailStack;
    registerDiffLeaveCheck(async () => {
      throw new Error('save failed');
    });

    await expect(openProjectExternally('b')).resolves.toBe('diffCommentUnsaved');

    expect(navigation.detailStack).toBe(stack);
    expect(error).toHaveBeenCalledTimes(1);
  });

  it('refuses for a dialog that opened while the comment was saving', async () => {
    const {
      openProjectExternally,
      selectProject,
      openDiffRoute,
      registerDiffLeaveCheck,
      navigation,
    } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const stack = navigation.detailStack;
    registerDiffLeaveCheck(async () => {
      addDialog();
      return true;
    });

    await expect(openProjectExternally('b')).resolves.toBe('dialogOpen');

    expect(navigation.detailStack).toBe(stack);
  });

  it('answers not found for a project deleted while the comment was saving', async () => {
    const {
      openProjectExternally,
      selectProject,
      openDiffRoute,
      registerDiffLeaveCheck,
      navigation,
    } = await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const stack = navigation.detailStack;
    registerDiffLeaveCheck(async () => {
      projectsStoreMock.projects = [{ id: 'a' }];
      return true;
    });

    await expect(openProjectExternally('b')).resolves.toBe('projectNotFound');

    expect(navigation.detailStack).toBe(stack);
  });

  it('answers not found for an id the window does not list', async () => {
    const { openProjectExternally, navigation } = await importNavigation();

    await expect(openProjectExternally('missing')).resolves.toBe('projectNotFound');

    expect(navigation.detailStack).toEqual([home]);
    expect(setStoreValue).not.toHaveBeenCalled();
  });

  it('answers not found for a project with a delete in flight, even on screen', async () => {
    const { openProjectExternally, selectProject, navigation } = await importNavigation();
    selectProject('a');
    deletingProjectIds.add('a');
    deletingProjectIds.add('b');

    await expect(openProjectExternally('b')).resolves.toBe('projectNotFound');
    await expect(openProjectExternally('a')).resolves.toBe('projectNotFound');

    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  it('answers not found ahead of an open dialog and without a diff save', async () => {
    const { openProjectExternally, selectProject, openDiffRoute, registerDiffLeaveCheck } =
      await importNavigation();
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const check = vi.fn().mockResolvedValue(true);
    registerDiffLeaveCheck(check);
    addDialog();

    await expect(openProjectExternally('missing')).resolves.toBe('projectNotFound');

    expect(check).not.toHaveBeenCalled();
  });

  it('waits for the first project list before deciding', async () => {
    projectsStoreMock.projects = [];
    projectsStoreMock.loaded = false;
    let finishLoad!: () => void;
    projectsStoreMock.whenLoaded.mockReturnValue(
      new Promise<void>((resolve) => (finishLoad = resolve))
    );
    const { openProjectExternally, navigation } = await importNavigation();
    let answer: string | null = null;

    const opening = openProjectExternally('a').then((value) => (answer = value));
    await Promise.resolve();
    await Promise.resolve();
    expect(answer).toBeNull();
    expect(navigation.detailStack).toEqual([home]);

    projectsStoreMock.projects = [{ id: 'a' }];
    projectsStoreMock.loaded = true;
    finishLoad();
    await opening;

    expect(answer).toBe('opened');
    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  it('opens the project when the list failed to load, as a window seed does', async () => {
    projectsStoreMock.projects = [];
    projectsStoreMock.loaded = false;
    const { openProjectExternally, navigation } = await importNavigation();

    await expect(openProjectExternally('a')).resolves.toBe('opened');

    expect(projectsStoreMock.whenLoaded).toHaveBeenCalledTimes(1);
    expect(navigation.detailStack).toEqual([projectRoute('a')]);
  });

  describe('with a stillWanted check', () => {
    it('navigates once the check says the request is still wanted', async () => {
      const { openProjectExternally, navigation } = await importNavigation();
      const stillWanted = vi.fn().mockResolvedValue(true);

      await expect(openProjectExternally('a', stillWanted)).resolves.toBe('opened');

      expect(stillWanted).toHaveBeenCalledTimes(1);
      expect(navigation.detailStack).toEqual([projectRoute('a')]);
    });

    it('leaves the window alone for a request withdrawn during the wait for the list', async () => {
      projectsStoreMock.projects = [];
      projectsStoreMock.loaded = false;
      let finishLoad!: () => void;
      projectsStoreMock.whenLoaded.mockReturnValue(
        new Promise<void>((resolve) => (finishLoad = resolve))
      );
      const { openProjectExternally, navigation } = await importNavigation();
      // The caller is waiting when the request is taken, and has timed out
      // by the time the list lands.
      let pending = true;
      const stillWanted = vi.fn(async () => pending);

      const opening = openProjectExternally('a', stillWanted);
      await Promise.resolve();
      await Promise.resolve();
      // Asked only at the end: an early yes would be stale by now.
      expect(stillWanted).not.toHaveBeenCalled();

      pending = false;
      projectsStoreMock.projects = [{ id: 'a' }];
      projectsStoreMock.loaded = true;
      finishLoad();

      await expect(opening).resolves.toBe('withdrawn');
      expect(stillWanted).toHaveBeenCalledTimes(1);
      expect(navigation.detailStack).toEqual([home]);
      expect(setStoreValue).not.toHaveBeenCalled();
    });

    it('leaves the window on the diff for a request withdrawn during the save', async () => {
      const {
        openProjectExternally,
        selectProject,
        openDiffRoute,
        registerDiffLeaveCheck,
        navigation,
      } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const stack = navigation.detailStack;
      let finishSave!: (saved: boolean) => void;
      const check = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
      registerDiffLeaveCheck(check);
      let pending = true;
      const stillWanted = vi.fn(async () => pending);

      const opening = openProjectExternally('b', stillWanted);
      await vi.waitFor(() => expect(check).toHaveBeenCalledTimes(1));
      expect(stillWanted).not.toHaveBeenCalled();

      // Superseded, or timed out, while the comment was being saved.
      pending = false;
      finishSave(true);

      await expect(opening).resolves.toBe('withdrawn');
      expect(stillWanted).toHaveBeenCalledTimes(1);
      // The comment is saved all the same; only the navigation is dropped.
      expect(navigation.detailStack).toBe(stack);
      expect(dispatchEvent).not.toHaveBeenCalled();
    });

    it('asks only after the save, and navigates when still wanted', async () => {
      const {
        openProjectExternally,
        selectProject,
        openDiffRoute,
        registerDiffLeaveCheck,
        navigation,
      } = await importNavigation();
      selectProject('a');
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      const order: string[] = [];
      registerDiffLeaveCheck(async () => {
        order.push('save');
        return true;
      });
      const stillWanted = vi.fn(async () => {
        order.push('stillWanted');
        return true;
      });

      await expect(openProjectExternally('b', stillWanted)).resolves.toBe('opened');

      expect(order).toEqual(['save', 'stillWanted']);
      expect(navigation.detailStack).toEqual([projectRoute('b')]);
    });

    it('does not ask when there is nothing to navigate', async () => {
      const { openProjectExternally, selectProject, openDiffRoute, registerDiffLeaveCheck } =
        await importNavigation();
      const stillWanted = vi.fn().mockResolvedValue(true);
      selectProject('a');

      // Already on screen, unknown to the window, and behind a dialog.
      await expect(openProjectExternally('a', stillWanted)).resolves.toBe('opened');
      await expect(openProjectExternally('missing', stillWanted)).resolves.toBe('projectNotFound');
      addDialog();
      await expect(openProjectExternally('b', stillWanted)).resolves.toBe('dialogOpen');
      dialogs = [];

      // And a comment that could not be saved.
      openDiffRoute({ branchId: 'b1', projectId: 'a' });
      registerDiffLeaveCheck(async () => false);
      await expect(openProjectExternally('b', stillWanted)).resolves.toBe('diffCommentUnsaved');

      expect(stillWanted).not.toHaveBeenCalled();
    });

    it('looks at the dialogs and the project again after the check', async () => {
      const { openProjectExternally, navigation } = await importNavigation();

      // The check is a round trip, and the window does not stand still for it.
      const opensDialog = vi.fn(async () => {
        addDialog();
        return true;
      });
      await expect(openProjectExternally('a', opensDialog)).resolves.toBe('dialogOpen');
      dialogs = [];

      const losesProject = vi.fn(async () => {
        projectsStoreMock.projects = [{ id: 'b' }];
        return true;
      });
      await expect(openProjectExternally('a', losesProject)).resolves.toBe('projectNotFound');

      expect(navigation.detailStack).toEqual([home]);
    });

    it('rejects, and navigates nowhere, when the check itself fails', async () => {
      const { openProjectExternally, navigation } = await importNavigation();
      const stillWanted = vi.fn().mockRejectedValue(new Error('ipc down'));

      await expect(openProjectExternally('a', stillWanted)).rejects.toThrow('ipc down');

      expect(navigation.detailStack).toEqual([home]);
    });
  });
});

describe('workspace layers', () => {
  // The workspace is two stacked layers (App.svelte): the home grid, mounted
  // once `homeLayerRetained` and active while `homeActive`, and the project
  // layer, holding the last project / repos base route in `projectLayer`.

  it('seeds a cold Tauri start at the root with nothing retained until the route settles', async () => {
    getStoreValue.mockResolvedValueOnce('p1');
    const { restoreInitialRoute, navigation } = await importNavigation();

    expect(navigation.homeActive).toBe(true);
    expect(navigation.homeLayerRetained).toBe(false);
    expect(navigation.projectLayer).toBeNull();

    await restoreInitialRoute();

    // Restored into a project: the grid was never shown, so it isn't retained.
    expect(navigation.homeActive).toBe(false);
    expect(navigation.homeLayerRetained).toBe(false);
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });
  });

  it('retains home once a start without a persisted project settles at the root', async () => {
    getStoreValue.mockResolvedValueOnce(null);
    const { restoreInitialRoute, navigation } = await importNavigation();

    await restoreInitialRoute();

    expect(navigation.homeActive).toBe(true);
    expect(navigation.homeLayerRetained).toBe(true);
    expect(navigation.projectLayer).toBeNull();
  });

  it('seeds the project layer from the web snapshot without retaining home', async () => {
    snapshotProjectId = 'p1';
    windowLabel = null;
    const { restoreInitialRoute, navigation } = await importNavigation();

    expect(navigation.homeActive).toBe(false);
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });

    await restoreInitialRoute();
    expect(navigation.homeLayerRetained).toBe(false);
  });

  it('goHome keeps the project layer and activates and retains home', async () => {
    const { restoreInitialRoute, selectProject, goHome, navigation } = await importNavigation();
    await restoreInitialRoute();
    selectProject('p1');
    const layer = navigation.projectLayer;

    goHome();

    expect(navigation.homeActive).toBe(true);
    expect(navigation.homeLayerRetained).toBe(true);
    expect(navigation.projectLayer).toBe(layer);
    expect(navigation.selectedProjectId).toBeNull();
  });

  it('keeps the layer object identity while the project is unchanged and replaces it otherwise', async () => {
    const { restoreInitialRoute, selectProject, goHome, showAllRepos, navigation } =
      await importNavigation();
    await restoreInitialRoute();

    selectProject('p1');
    const p1 = navigation.projectLayer;
    expect(p1).toEqual({ kind: 'project', projectId: 'p1' });
    goHome();
    selectProject('p1');
    expect(navigation.projectLayer).toBe(p1);

    goHome();
    selectProject('p2');
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p2' });
    expect(navigation.homeActive).toBe(false);

    showAllRepos();
    const repos = navigation.projectLayer;
    expect(repos).toEqual({ kind: 'repos' });
    goHome();
    showAllRepos();
    expect(navigation.projectLayer).toBe(repos);
  });

  it('shows a pinned board in the project layer, and releases it only under home', async () => {
    const {
      restoreInitialRoute,
      selectBoard,
      selectProject,
      goHome,
      releaseBoardLayer,
      navigation,
    } = await importNavigation();
    await restoreInitialRoute();

    selectBoard('g1');
    expect(navigation.currentRoute).toEqual({ kind: 'board', groupId: 'g1' });
    expect(navigation.homeActive).toBe(false);
    expect(navigation.selectedProjectId).toBeNull();
    expect(navigation.showReposList).toBe(false);
    const board = navigation.projectLayer;
    expect(board).toEqual({ kind: 'board', groupId: 'g1' });
    // The board is never the persisted route.
    expect(setStoreValue).not.toHaveBeenCalledWith('last-viewed-project', 'g1');

    // On screen, the view itself leaves for home; a release here is ignored.
    releaseBoardLayer('g1');
    expect(navigation.projectLayer).toBe(board);

    // Another board is the workspace route in its place, not over it.
    selectBoard('g2');
    expect(navigation.detailStack.map((route) => route.kind)).toEqual(['board']);
    expect(navigation.projectLayer).toEqual({ kind: 'board', groupId: 'g2' });

    // So is a project opened from the board, as from the repos view.
    selectProject('p1');
    expect(navigation.detailStack.map((route) => route.kind)).toEqual(['project']);

    selectBoard('g2');
    goHome();
    const retained = navigation.projectLayer;
    expect(retained).toEqual({ kind: 'board', groupId: 'g2' });
    releaseBoardLayer('other');
    expect(navigation.projectLayer).toBe(retained);
    releaseBoardLayer('g2');
    expect(navigation.projectLayer).toBeNull();
  });

  it('leaves a board that is no longer offered, on screen, under settings or retained', async () => {
    const { restoreInitialRoute, selectBoard, openSettings, goHome, leaveBoard, navigation } =
      await importNavigation();
    await restoreInitialRoute();

    // On screen: home.
    selectBoard('g1');
    leaveBoard('g1');
    expect(navigation.detailStack.map((route) => route.kind)).toEqual(['projects']);
    expect(navigation.homeActive).toBe(true);
    expect(navigation.projectLayer).toBeNull();
    expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', null);

    // Under settings (where the unpinning was done): settings stays open,
    // over the home grid, and the board is no longer a step Back.
    selectBoard('g1');
    openSettings('tags');
    leaveBoard('g1');
    expect(navigation.detailStack.map((route) => route.kind)).toEqual(['projects', 'settings']);
    expect(navigation.currentRoute.kind).toBe('settings');
    expect(navigation.homeActive).toBe(true);
    expect(navigation.projectLayer).toBeNull();
    expect(navigation.history.entries).toEqual([
      [{ kind: 'projects' }],
      [{ kind: 'projects' }, { kind: 'settings', section: 'tags' }],
    ]);

    // Retained under home: the layer is released, the view untouched.
    goHome();
    selectBoard('g2');
    goHome();
    expect(navigation.projectLayer).toEqual({ kind: 'board', groupId: 'g2' });
    leaveBoard('g2');
    expect(navigation.projectLayer).toBeNull();
    expect(navigation.detailStack.map((route) => route.kind)).toEqual(['projects']);

    // Another group's board: nothing changes.
    selectBoard('g3');
    leaveBoard('g2');
    expect(navigation.currentRoute).toEqual({ kind: 'board', groupId: 'g3' });
  });

  it('keeps the history bounded through board ↔ All repos round trips', async () => {
    const { MAX_HISTORY_ENTRIES } = await import('./navigationHistory');
    const { restoreInitialRoute, selectBoard, showAllRepos, selectProject, navigation } =
      await importNavigation();
    await restoreInitialRoute();

    // Each is the whole workspace route, and each trip between them a visit.
    selectBoard('g1');
    showAllRepos();
    expect(navigation.detailStack.map((route) => route.kind)).toEqual(['repos']);
    selectBoard('g1');
    expect(navigation.detailStack.map((route) => route.kind)).toEqual(['board']);
    expect(navigation.history.entries).toHaveLength(4);

    for (let i = 0; i < MAX_HISTORY_ENTRIES; i++) {
      showAllRepos();
      selectBoard('g1');
    }
    expect(navigation.history.entries).toHaveLength(MAX_HISTORY_ENTRIES);
    expect(navigation.currentRoute).toEqual({ kind: 'board', groupId: 'g1' });
    expect(navigation.canGoForward).toBe(false);

    // All repos opened from a project is the route in its place too.
    selectProject('p1');
    showAllRepos();
    expect(navigation.detailStack.map((route) => route.kind)).toEqual(['repos']);
    expect(navigation.selectedProjectId).toBeNull();
  });

  it('takes an unpinned board out of the history, landing where Back would', async () => {
    vi.stubGlobal('document', { querySelectorAll: () => [] });
    const {
      restoreInitialRoute,
      selectBoard,
      showAllRepos,
      selectProject,
      leaveBoard,
      goBack,
      navigation,
    } = await importNavigation();
    await restoreInitialRoute();

    // Every entry for the board goes, and other groups' boards stay. The
    // view on screen was the board, so the entry before it takes its place.
    selectBoard('g1');
    showAllRepos();
    selectBoard('g1');
    selectBoard('g2');
    await goBack();
    leaveBoard('g1');
    expect(navigation.history.entries).toEqual([
      [{ kind: 'projects' }],
      [{ kind: 'repos' }],
      [{ kind: 'board', groupId: 'g2' }],
    ]);
    expect(navigation.currentRoute).toEqual({ kind: 'repos' });
    expect(navigation.projectLayer).toEqual({ kind: 'repos' });
    expect(navigation.canGoForward).toBe(true);

    // A board ahead of the view on screen goes without moving it.
    leaveBoard('g2');
    expect(navigation.currentRoute).toEqual({ kind: 'repos' });
    expect(navigation.canGoForward).toBe(false);

    // A board opened from a project lands back on the project, which is
    // persisted.
    selectProject('p1');
    selectBoard('g1');
    setStoreValue.mockClear();
    leaveBoard('g1');
    expect(navigation.detailStack).toEqual([{ kind: 'project', projectId: 'p1' }]);
    expect(navigation.selectedProjectId).toBe('p1');
    expect(navigation.homeActive).toBe(false);
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });
    expect(setStoreValue).toHaveBeenLastCalledWith('last-viewed-project', 'p1');
  });

  it('keeps both layers through settings and a diff over home', async () => {
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const {
      restoreInitialRoute,
      selectProject,
      goHome,
      openSettings,
      closeSettings,
      openDiffRoute,
      closeDiffRouteIfCurrent,
      navigation,
    } = await importNavigation();
    await restoreInitialRoute();
    selectProject('p1');
    goHome();
    const layer = navigation.projectLayer;

    openSettings();
    expect(navigation.workspaceParked).toBe(true);
    expect(navigation.homeActive).toBe(true);
    expect(navigation.projectLayer).toBe(layer);
    closeSettings();

    // A diff that takes another's place doesn't mount the project it names
    // under it: the layers follow the base route, which is still home.
    openDiffRoute({ branchId: 'b1' });
    expect(navigation.homeActive).toBe(true);
    expect(navigation.projectLayer).toBe(layer);
    openDiffRoute({ branchId: 'b2', projectId: 'p2' });
    expect(navigation.currentRoute.kind).toBe('diff');
    expect(navigation.selectedProjectId).toBe('p2');
    expect(navigation.homeActive).toBe(true);
    expect(navigation.projectLayer).toBe(layer);
    closeDiffRouteIfCurrent(navigation.currentRoute as DiffDetailRoute);
    expect(navigation.currentRoute).toEqual({ kind: 'projects' });
    expect(navigation.homeActive).toBe(true);
  });

  it('keeps home inactive and unparks back to the project under settings and a diff', async () => {
    vi.stubGlobal('window', { dispatchEvent: vi.fn() });
    const {
      restoreInitialRoute,
      selectProject,
      openSettings,
      closeSettings,
      openDiffRoute,
      closeDiffRouteIfCurrent,
      navigation,
    } = await importNavigation();
    await restoreInitialRoute();
    selectProject('p1');
    const layer = navigation.projectLayer;

    openDiffRoute({ branchId: 'b1', projectId: 'p1' });
    openSettings();
    expect(navigation.homeActive).toBe(false);
    expect(navigation.projectLayer).toBe(layer);
    closeSettings();
    closeDiffRouteIfCurrent(navigation.currentRoute as DiffDetailRoute);
    expect(navigation.currentRoute).toEqual({ kind: 'project', projectId: 'p1' });
    expect(navigation.homeActive).toBe(false);
    expect(navigation.projectLayer).toBe(layer);
  });

  it('releases the project layer only on home with a matching id', async () => {
    const { restoreInitialRoute, selectProject, goHome, releaseProjectLayer, navigation } =
      await importNavigation();
    await restoreInitialRoute();
    selectProject('p1');

    // On the project itself, the layer is the view on screen.
    releaseProjectLayer('p1');
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });

    goHome();
    releaseProjectLayer('p2');
    expect(navigation.projectLayer).toEqual({ kind: 'project', projectId: 'p1' });
    releaseProjectLayer('p1');
    expect(navigation.projectLayer).toBeNull();

    // Nothing to release: a no-op rather than a throw.
    releaseProjectLayer('p1');
    expect(navigation.projectLayer).toBeNull();
  });
});

describe('followWhileVisible', () => {
  // Under vitest $state.raw is a plain variable and Svelte tracks nothing, so
  // the source and the layer's `visible` are small signals here. Each effect
  // run records the signals it reads outside `untrack`. $effect.pre runs once
  // when it is set up, like at mount, and `flush` reruns, after its cleanup,
  // only the effects whose recorded reads have changed since, the way Svelte
  // would. A read the effect stops tracking then fails a test.
  type Signal<T> = { value: T; readonly version: number };
  type Effect = {
    run: () => void | (() => void);
    cleanup?: void | (() => void);
    reads: Map<Signal<unknown>, number>;
  };
  let effects: Effect[];
  let reads: Map<Signal<unknown>, number> | null;
  let layerVisible: Signal<boolean>;

  function signal<T>(initial: T): Signal<T> {
    let stored = initial;
    let version = 0;
    const self: Signal<T> = {
      get value() {
        reads?.set(self, version);
        return stored;
      },
      set value(next) {
        if (Object.is(next, stored)) return;
        stored = next;
        version++;
      },
      get version() {
        return version;
      },
    };
    return self;
  }

  function untracked<T>(fn: () => T): T {
    const outer = reads;
    reads = null;
    try {
      return fn();
    } finally {
      reads = outer;
    }
  }

  function execute(effect: Effect) {
    const outer = reads;
    reads = effect.reads = new Map();
    try {
      effect.cleanup = effect.run();
    } finally {
      reads = outer;
    }
  }

  beforeEach(() => {
    vi.useFakeTimers();
    effects = [];
    reads = null;
    layerVisible = signal(true);
    const identity = (value: unknown) => value;
    const track = (run: Effect['run']) => {
      const effect: Effect = { run, reads: new Map() };
      effects.push(effect);
      execute(effect);
    };
    vi.stubGlobal('$state', Object.assign(identity, { raw: identity }));
    vi.stubGlobal('$effect', Object.assign(track, { pre: track }));
    vi.doMock('svelte', async (importOriginal) => ({
      ...(await importOriginal<typeof import('svelte')>()),
      untrack: untracked,
    }));
    vi.doMock('./workspaceLayer', () => ({
      useWorkspaceLayer: () => ({
        get visible() {
          return layerVisible.value;
        },
      }),
    }));
  });

  afterEach(() => {
    vi.doUnmock('svelte');
    vi.doUnmock('./workspaceLayer');
  });

  function flush() {
    for (const effect of effects) {
      const changed = [...effect.reads].some(([read, version]) => read.version !== version);
      if (!changed) continue;
      untracked(() => effect.cleanup?.());
      execute(effect);
    }
  }

  function unmount() {
    for (const effect of effects) untracked(() => effect.cleanup?.());
    effects = [];
  }

  it('follows the source in the same flush while the layer is visible', async () => {
    const { followWhileVisible } = await importNavigation();
    const source = signal('a');
    const followed = followWhileVisible(() => source.value);
    expect(followed.current).toBe('a');

    source.value = 'b';
    flush();
    expect(followed.current).toBe('b');
    expect(vi.getTimerCount()).toBe(0);
  });

  it('follows the source in a hidden layer once it stops changing', async () => {
    const { followWhileVisible, HIDDEN_FOLLOW_DELAY_MS } = await importNavigation();
    const source = signal('');
    const followed = followWhileVisible(() => source.value);
    layerVisible.value = false;
    flush();

    source.value = 's';
    flush();
    await vi.advanceTimersByTimeAsync(HIDDEN_FOLLOW_DELAY_MS - 1);
    source.value = 'sl';
    flush();
    // The second keystroke restarted the wait.
    await vi.advanceTimersByTimeAsync(HIDDEN_FOLLOW_DELAY_MS - 1);
    expect(followed.current).toBe('');

    await vi.advanceTimersByTimeAsync(1);
    expect(followed.current).toBe('sl');
  });

  it('catches up in the flush that shows the layer without waiting', async () => {
    const { followWhileVisible } = await importNavigation();
    const source = signal('slow');
    const followed = followWhileVisible(() => source.value);
    layerVisible.value = false;
    flush();

    source.value = '';
    flush();
    layerVisible.value = true;
    flush();

    expect(followed.current).toBe('');
    // The hidden wait was cancelled rather than left to fire later.
    expect(vi.getTimerCount()).toBe(0);
  });

  it('seeds from the source when set up in a hidden layer', async () => {
    layerVisible.value = false;
    const { followWhileVisible } = await importNavigation();
    const followed = followWhileVisible(() => 'seed');

    expect(followed.current).toBe('seed');
  });

  it('cancels a pending hidden wait when the view unmounts', async () => {
    const { followWhileVisible, HIDDEN_FOLLOW_DELAY_MS } = await importNavigation();
    const source = signal('');
    const followed = followWhileVisible(() => source.value);
    layerVisible.value = false;
    flush();
    source.value = 's';
    flush();
    expect(vi.getTimerCount()).toBe(1);

    unmount();

    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(HIDDEN_FOLLOW_DELAY_MS);
    expect(followed.current).toBe('');
  });
});
