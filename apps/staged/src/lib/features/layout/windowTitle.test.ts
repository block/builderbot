import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// ── Mock plumbing ──
//
// windowTitle.ts imports parseRepoFilterKey from projectFilters.svelte.ts,
// which also instantiates the filter-store singleton and pulls in the global
// data stores — mock those out and stub the runes the way projectFilters.test.ts
// does. The transport is mocked so applyWindowTitle's IPC is observable.

let setTitle: ReturnType<typeof vi.fn>;

async function importWindowTitle() {
  return await import('./windowTitle');
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  const derived = (value: unknown) => value;
  derived.by = (compute: () => unknown) => compute();
  vi.stubGlobal('$derived', derived);
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: { projects: [], reposByProject: new Map() },
  }));
  vi.doMock('../../stores/projectState.svelte', () => ({
    projectStateStore: { isUnread: () => false },
  }));
  vi.doMock('../projects/projectStatus', () => ({ getProjectStatus: vi.fn() }));

  setTitle = vi.fn().mockResolvedValue(undefined);
  vi.doMock('../../transport', () => ({ getWindowSync: () => ({ setTitle }) }));
});

afterEach(() => {
  vi.doUnmock('../../stores/projectsData.svelte');
  vi.doUnmock('../../stores/projectState.svelte');
  vi.doUnmock('../projects/projectStatus');
  vi.doUnmock('../../transport');
  vi.unstubAllGlobals();
});

// ── Tests ──

describe('formatWindowTitle', () => {
  it('falls back to the default title when nothing is filtered', async () => {
    const { formatWindowTitle, DEFAULT_WINDOW_TITLE } = await importWindowTitle();
    expect(DEFAULT_WINDOW_TITLE).toBe('Staged');
    expect(formatWindowTitle(new Set())).toBe('Staged');
  });

  it('orders status filters Unread-then-Running regardless of selection order', async () => {
    const { formatWindowTitle } = await importWindowTitle();
    expect(formatWindowTitle(new Set(['unread']))).toBe('Unread');
    expect(formatWindowTitle(new Set(['running']))).toBe('Running');
    expect(formatWindowTitle(new Set(['unread', 'running']))).toBe('Unread · Running');
    expect(formatWindowTitle(new Set(['running', 'unread']))).toBe('Unread · Running');
  });

  it('names a repo filter by its emphasised segment, not its full path', async () => {
    const { formatWindowTitle } = await importWindowTitle();
    expect(formatWindowTitle(new Set(['repo:block/mark:']))).toBe('mark');
  });

  it('emphasises the whole subpath, matching the repo chips', async () => {
    const { formatWindowTitle } = await importWindowTitle();
    expect(formatWindowTitle(new Set(['repo:block/builderbot:apps/staged']))).toBe('apps/staged');
    expect(formatWindowTitle(new Set(['repo:block/mark:ui']))).toBe('ui');
  });

  it('joins status filters and repos, repos sorted by displayed label', async () => {
    const { formatWindowTitle } = await importWindowTitle();
    expect(formatWindowTitle(new Set(['repo:block/zulu:', 'unread', 'repo:acme/alpha:']))).toBe(
      'Unread · alpha, zulu'
    );
  });

  it('collapses everything past the second repo into "+N more"', async () => {
    const { formatWindowTitle } = await importWindowTitle();
    const four = new Set([
      'repo:block/alpha:',
      'repo:block/bravo:',
      'repo:block/charlie:',
      'repo:block/delta:',
    ]);
    expect(formatWindowTitle(four)).toBe('alpha, bravo, +2 more');
    expect(formatWindowTitle(new Set(['unread', ...four]))).toBe('Unread · alpha, bravo, +2 more');
  });

  it('still names a stale repo filter whose repo no longer exists', async () => {
    // Derived from the filter key alone, so it never waits on (or is emptied
    // by) the repo-list hydration the chips read.
    const { formatWindowTitle } = await importWindowTitle();
    expect(formatWindowTitle(new Set(['repo:deleted/repo:']))).toBe('repo');
  });

  it('ignores unrecognized keys rather than treating them as repos', async () => {
    const { formatWindowTitle } = await importWindowTitle();
    expect(formatWindowTitle(new Set(['archived']))).toBe('Staged');
  });
});

describe('applyWindowTitle', () => {
  it('issues no IPC for the default title the window already has', async () => {
    const { applyWindowTitle, DEFAULT_WINDOW_TITLE } = await importWindowTitle();
    await applyWindowTitle(DEFAULT_WINDOW_TITLE);
    expect(setTitle).not.toHaveBeenCalled();
  });

  it('skips repeats of the title it last requested', async () => {
    const { applyWindowTitle } = await importWindowTitle();
    await applyWindowTitle('Unread');
    await applyWindowTitle('Unread');
    expect(setTitle.mock.calls).toEqual([['Unread']]);
  });

  it('lets a superseded title be dropped instead of landing last', async () => {
    const { applyWindowTitle } = await importWindowTitle();
    const first = applyWindowTitle('Unread');
    const second = applyWindowTitle('Running');
    await Promise.all([first, second]);
    expect(setTitle.mock.calls).toEqual([['Running']]);
  });

  it('serializes overlapping calls so the newest title lands last', async () => {
    const { applyWindowTitle } = await importWindowTitle();
    let releaseFirst: () => void = () => {};
    setTitle.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          releaseFirst = resolve;
        })
    );

    const first = applyWindowTitle('Unread');
    await Promise.resolve();
    expect(setTitle.mock.calls).toEqual([['Unread']]);

    const second = applyWindowTitle('Running');
    releaseFirst();
    await Promise.all([first, second]);
    expect(setTitle.mock.calls).toEqual([['Unread'], ['Running']]);
  });

  it('swallows a rejected setTitle so the effect never sees it', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { applyWindowTitle } = await importWindowTitle();
    setTitle.mockRejectedValueOnce(new Error('permission denied'));

    await expect(applyWindowTitle('Unread')).resolves.toBeUndefined();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
