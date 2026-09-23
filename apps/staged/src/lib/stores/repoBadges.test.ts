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
});
