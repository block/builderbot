import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The real project state store and session registry — both rune modules;
// under plain vitest `$state` is stubbed as identity, which is all their
// Map-and-counter state needs — minus the dock badge and the persistent store.

vi.mock('../transport', () => ({
  getWindowSync: () => ({ setBadgeCount: async () => {} }),
}));
vi.mock('../shared/persistentStore', () => ({
  getStoreValue: async () => undefined,
  setStoreValue: async () => {},
  listenToStoreKeyChange: (
    _key: string,
    _callback: (value: unknown) => void,
    options?: { onEstablished?: () => void }
  ) => {
    options?.onEstablished?.();
    return () => {};
  },
}));

async function freshStores() {
  vi.resetModules();
  const { sessionRegistry } = await import('./sessionRegistry.svelte');
  const { projectStateStore } = await import('./projectState.svelte');
  return { sessionRegistry, projectStateStore };
}

beforeEach(() => {
  vi.stubGlobal('$state', (value: unknown) => value);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('projectStateStore.getRunningProjectSessionIds', () => {
  it('lists the project’s own running sessions, not those on its branches', async () => {
    const { sessionRegistry, projectStateStore } = await freshStores();
    // A project-note session has no branch; branch sessions name theirs.
    sessionRegistry.register('plan-1', 'p1', 'other');
    sessionRegistry.register('commit-1', 'p1', 'commit', 'b1');
    sessionRegistry.register('note-1', 'p1', 'note', 'b2');
    sessionRegistry.register('plan-2', 'p1', 'other');
    sessionRegistry.register('plan-other', 'p2', 'other');
    for (const id of ['plan-1', 'commit-1', 'note-1', 'plan-2']) {
      projectStateStore.addRunningSession('p1', id);
    }
    projectStateStore.addRunningSession('p2', 'plan-other');

    expect(projectStateStore.getRunningProjectSessionIds('p1')).toEqual(['plan-1', 'plan-2']);
    expect(projectStateStore.hasRunningSessions('p1')).toBe(true);
    expect(projectStateStore.getRunningProjectSessionIds('p2')).toEqual(['plan-other']);
    expect(projectStateStore.getRunningProjectSessionIds('unknown')).toEqual([]);
  });

  it('follows sessions ending, and reads a branch-only project as having none of its own', async () => {
    const { sessionRegistry, projectStateStore } = await freshStores();
    sessionRegistry.register('plan-1', 'p1', 'other');
    sessionRegistry.register('commit-1', 'p1', 'commit', 'b1');
    projectStateStore.addRunningSession('p1', 'plan-1');
    projectStateStore.addRunningSession('p1', 'commit-1');

    projectStateStore.removeRunningSession('p1', 'plan-1');
    expect(projectStateStore.getRunningProjectSessionIds('p1')).toEqual([]);
    // Work on a sibling branch alone does not make the project itself busy.
    expect(projectStateStore.hasRunningSessions('p1')).toBe(true);
  });
});
