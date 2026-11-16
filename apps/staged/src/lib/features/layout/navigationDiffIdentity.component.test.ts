import { beforeEach, describe, expect, it, vi } from 'vitest';

// navigation.test.ts runs navigation.svelte.ts with `$state` stubbed as
// identity, so what it holds there is the plain objects it was given. In
// the app `navigation` is a deep state proxy, and a proxy is made per
// holder: a plain object put into two history entries reads back out of
// them as two different proxies. Diff routes are compared by identity, so
// the refresh that swaps a diff route into every entry sharing it
// (replaceDiffRoute) has to hand them all one proxy. This file is compiled
// with the real runes (the component project) to hold it to that.

const refreshDiffRoute = vi.hoisted(() =>
  vi.fn(async (route: object) => ({ status: 'current', route: { ...route, refreshed: true } }))
);

vi.mock('../../transport', () => ({ getWindowLabel: () => 'main' }));
vi.mock('../../shared/persistentStore', () => ({
  getStoreValue: vi.fn(async () => null),
  initPersistentStore: vi.fn(async () => undefined),
  setStoreValue: vi.fn(async () => undefined),
}));
vi.mock('../../shared/webSnapshot', () => ({
  SNAPSHOT_KEYS: { lastProject: 'staged:boot:last-project' },
  readSnapshot: () => null,
  writeSnapshot: vi.fn(),
  clearSnapshot: vi.fn(),
}));
vi.mock('../../commands', () => ({ takeWindowSeed: vi.fn(async () => null) }));
vi.mock('../../stores/projectState.svelte', () => ({
  projectStateStore: { isUnread: () => false, markAsRead: vi.fn() },
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: { projects: [], loaded: false, isProjectDeleting: () => false },
}));
vi.mock('../projects/projectsListViewState.svelte', () => ({
  requestProjectsListRestore: vi.fn(),
}));
vi.mock('../../stores/tags.svelte', () => ({
  tagsStore: { loaded: false, catalog: { groupsById: new Map() } },
}));
vi.mock('../../shared/viewport.svelte', () => ({ viewport: { isMobile: false } }));
vi.mock('./diffRouteRefresh', () => ({ refreshDiffRoute }));

import {
  closeSettings,
  goBack,
  goForward,
  navigation,
  openDiffRoute,
  openSettings,
  selectProject,
} from './navigation.svelte';

const diffEvents: string[] = [];
window.addEventListener('staged:diff-route-replaced', () => diffEvents.push('replaced'));
window.addEventListener('staged:diff-route-popped', () => diffEvents.push('popped'));

beforeEach(() => {
  diffEvents.length = 0;
  refreshDiffRoute.mockClear();
});

describe('a refreshed diff route, in the compiled navigation state', () => {
  it('is one route in every entry over it, so steps between them stay silent', async () => {
    selectProject('a');
    openDiffRoute({ branchId: 'b1', projectId: 'a' });
    const opened = navigation.currentRoute;
    openSettings('general');
    // The state really is proxied here: the two entries share the route.
    expect(navigation.history.entries[2][1]).toBe(opened);
    expect(navigation.history.entries[3][1]).toBe(opened);
    selectProject('b');
    diffEvents.length = 0;

    // Back onto Settings over the diff, which is asked about on the way.
    await expect(goBack()).resolves.toBe(true);
    expect(refreshDiffRoute).toHaveBeenCalledTimes(1);
    const refreshed = navigation.detailStack[1];
    expect(refreshed).toMatchObject({ kind: 'diff', branchId: 'b1', refreshed: true });
    expect(refreshed).not.toBe(opened);
    expect(navigation.history.entries[2][1]).toBe(refreshed);
    expect(navigation.history.entries[3][1]).toBe(refreshed);

    // Back onto the diff beneath and Forward again: not asked about, and
    // no diff left the screen.
    await expect(goBack()).resolves.toBe(true);
    expect(navigation.currentRoute).toBe(refreshed);
    await expect(goForward()).resolves.toBe(true);
    expect(navigation.currentRoute).toEqual({ kind: 'settings', section: 'general' });
    expect(refreshDiffRoute).toHaveBeenCalledTimes(1);
    expect(diffEvents).toEqual([]);

    // Closing Settings steps back onto the diff instead of adding an entry.
    closeSettings();
    expect(navigation.currentRoute).toBe(refreshed);
    expect(navigation.history.entries).toHaveLength(5);
    expect(navigation.history.index).toBe(2);
    expect(navigation.canGoForward).toBe(true);
    expect(diffEvents).toEqual([]);
  });
});
