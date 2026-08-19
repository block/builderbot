import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// ── Mock plumbing ──

let windowLabel: string | null;
let setStoreValue: ReturnType<typeof vi.fn>;
let writeSnapshot: ReturnType<typeof vi.fn>;
let clearSnapshot: ReturnType<typeof vi.fn>;

async function importNavigation() {
  return await import('./navigation.svelte');
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (projectsData.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);

  windowLabel = 'main';
  setStoreValue = vi.fn().mockResolvedValue(undefined);
  writeSnapshot = vi.fn();
  clearSnapshot = vi.fn();

  vi.doMock('../../transport', () => ({
    getWindowLabel: () => windowLabel,
  }));
  vi.doMock('../../shared/persistentStore', () => ({
    getStoreValue: vi.fn().mockResolvedValue(null),
    setStoreValue,
  }));
  vi.doMock('../../shared/webSnapshot', () => ({
    SNAPSHOT_KEYS: { lastProject: 'staged:boot:last-project' },
    readSnapshot: () => null,
    writeSnapshot,
    clearSnapshot,
  }));
  vi.doMock('../../commands', () => ({ takeWindowSeed: vi.fn().mockResolvedValue(null) }));
  vi.doMock('../../stores/projectState.svelte', () => ({
    projectStateStore: { isUnread: () => false, markAsRead: vi.fn() },
  }));
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: { projects: [], loaded: false, ensureLoaded: vi.fn() },
  }));
  vi.doMock('../projects/projectsListViewState.svelte', () => ({
    requestProjectsListRestore: vi.fn(),
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetAllMocks();
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
