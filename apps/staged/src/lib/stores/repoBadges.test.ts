import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RepoBadge } from '../types';
import type { SwrResult } from '../cache';

function badge(overrides: Partial<RepoBadge> = {}): RepoBadge {
  return {
    githubRepo: 'org/repo',
    subpath: '',
    shortName: 'repo',
    hue: 120,
    createdAt: 0,
    pinned: false,
    pinSortOrder: null,
    defaultBranch: 'main',
    ...overrides,
  };
}

function swr<T>(data: T, revalidating: Promise<T> | null = null): SwrResult<T> {
  return { data, revalidating };
}

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

let getAllRepoBadges: ReturnType<typeof vi.fn>;
let ensureRepoBadges: ReturnType<typeof vi.fn>;
let updateRepoBadge: ReturnType<typeof vi.fn>;

async function importStore() {
  const { repoBadgeStore } = await import('./repoBadges.svelte');
  return repoBadgeStore;
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  getAllRepoBadges = vi.fn().mockResolvedValue(swr([badge()]));
  ensureRepoBadges = vi.fn().mockResolvedValue([]);
  updateRepoBadge = vi.fn();
  vi.doMock('../commands', () => ({
    getAllRepoBadges,
    ensureRepoBadges,
    updateRepoBadge,
  }));
  vi.doMock('../features/agents/agent.svelte', () => ({
    agentState: { providers: [] },
  }));
  vi.doMock('../features/settings/preferences.svelte', () => ({
    getPreferredAgent: () => null,
  }));
});

afterEach(() => {
  vi.doUnmock('../commands');
  vi.doUnmock('../features/agents/agent.svelte');
  vi.doUnmock('../features/settings/preferences.svelte');
  vi.unstubAllGlobals();
});

describe('repoBadgeStore', () => {
  it('applies cached badge data immediately', async () => {
    const store = await importStore();

    await store.loadAll();

    expect(store.lookup('org/repo', '')?.shortName).toBe('repo');
  });

  it('applies SWR revalidation results when they resolve', async () => {
    let resolveFresh!: (badges: RepoBadge[]) => void;
    getAllRepoBadges.mockResolvedValueOnce(
      swr(
        [badge({ shortName: 'old' })],
        new Promise<RepoBadge[]>((resolve) => {
          resolveFresh = resolve;
        })
      )
    );
    const store = await importStore();

    await store.loadAll();
    expect(store.lookup('org/repo', '')?.shortName).toBe('old');

    resolveFresh([badge({ shortName: 'new' })]);
    await vi.waitFor(() => {
      expect(store.lookup('org/repo', '')?.shortName).toBe('new');
    });
  });

  it('passes force refresh through to getAllRepoBadges', async () => {
    const store = await importStore();

    await store.loadAll({ force: true });

    expect(getAllRepoBadges).toHaveBeenCalledWith({ force: true });
  });

  it('ignores stale load results that resolve after a newer load', async () => {
    let resolveStale!: (value: SwrResult<RepoBadge[]>) => void;
    getAllRepoBadges.mockReturnValueOnce(
      new Promise<SwrResult<RepoBadge[]>>((resolve) => {
        resolveStale = resolve;
      })
    );
    getAllRepoBadges.mockResolvedValueOnce(swr([badge({ shortName: 'fresh' })]));
    const store = await importStore();

    const staleLoad = store.loadAll();
    await store.loadAll({ force: true });
    expect(store.lookup('org/repo', '')?.shortName).toBe('fresh');

    resolveStale(swr([badge({ shortName: 'stale' })]));
    await staleLoad;

    expect(store.lookup('org/repo', '')?.shortName).toBe('fresh');
  });

  it('ignores stale SWR revalidation results after a newer load', async () => {
    let resolveStaleFresh!: (badges: RepoBadge[]) => void;
    getAllRepoBadges.mockResolvedValueOnce(
      swr(
        [badge({ shortName: 'old' })],
        new Promise<RepoBadge[]>((resolve) => {
          resolveStaleFresh = resolve;
        })
      )
    );
    getAllRepoBadges.mockResolvedValueOnce(swr([badge({ shortName: 'fresh' })]));
    const store = await importStore();

    await store.loadAll();
    await store.loadAll({ force: true });
    expect(store.lookup('org/repo', '')?.shortName).toBe('fresh');

    resolveStaleFresh([badge({ shortName: 'stale-swr' })]);
    await tick();

    expect(store.lookup('org/repo', '')?.shortName).toBe('fresh');
  });

  it('ignores stale load results after a badge update', async () => {
    let resolveStaleFresh!: (badges: RepoBadge[]) => void;
    getAllRepoBadges.mockResolvedValueOnce(
      swr(
        [badge({ shortName: 'old' })],
        new Promise<RepoBadge[]>((resolve) => {
          resolveStaleFresh = resolve;
        })
      )
    );
    updateRepoBadge.mockResolvedValueOnce(badge({ shortName: 'custom', hue: 180 }));
    const store = await importStore();

    await store.loadAll();
    await store.update('org/repo', '', 'custom', 180);
    expect(store.lookup('org/repo', '')?.shortName).toBe('custom');

    resolveStaleFresh([badge({ shortName: 'stale-swr' })]);
    await tick();

    expect(store.lookup('org/repo', '')?.shortName).toBe('custom');
  });
});
