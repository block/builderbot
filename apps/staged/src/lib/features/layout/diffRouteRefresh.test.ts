import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiffDetailRoute } from './navigation.svelte';

// What a history step asks about the diff entry it is about to land on:
// whether its branch and review still exist, and what the branch looks like
// now. The backend is replaced by the three reads it makes.

let getBranch: ReturnType<typeof vi.fn>;
/** The timeline read, by the branch cards' rule: `cached` when there is
 *  one, and `fresh` unless the cached one was fetched moments ago. */
let getBranchTimelineWithRevalidation: ReturnType<typeof vi.fn>;
/** A timeline that is not cached: the read goes to the backend. */
const uncached = (fresh: Promise<unknown>) => ({ cached: null, fresh });
let getReview: ReturnType<typeof vi.fn>;
let projects: Array<{ id: string; name: string }>;

const commit = (sha: string) => ({ sha, subject: `commit ${sha}` });
const branch = (overrides: Record<string, unknown> = {}) => ({
  id: 'b1',
  projectId: 'p1',
  branchName: 'feature',
  baseBranch: 'origin/main',
  ...overrides,
});

/** A diff of the whole branch, as a branch card opens one. */
const opened = (overrides: Partial<DiffDetailRoute> = {}): DiffDetailRoute => ({
  kind: 'diff',
  branchId: 'b1',
  projectId: 'p1',
  scope: 'branch',
  beforeLabel: 'main',
  afterLabel: 'feature',
  commits: [commit('old')] as never,
  baseBranchLabel: 'main',
  branchLabel: 'feature',
  projectName: 'Project',
  githubRepo: 'org/repo',
  subpath: 'apps/x',
  ...overrides,
});

async function importRefresh() {
  return await import('./diffRouteRefresh');
}

beforeEach(() => {
  vi.resetModules();
  getBranch = vi.fn().mockResolvedValue(branch());
  getBranchTimelineWithRevalidation = vi.fn(() =>
    uncached(Promise.resolve({ commits: [commit('old')] }))
  );
  getReview = vi.fn().mockResolvedValue({ id: 'r1' });
  projects = [{ id: 'p1', name: 'Project' }];
  vi.doMock('../../commands', () => ({
    getBranch,
    getBranchTimelineWithRevalidation,
    getReview,
  }));
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: {
      get projects() {
        return projects;
      },
    },
  }));
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('refreshDiffRoute', () => {
  it('says the branch is gone when the backend has none under the id', async () => {
    getBranch.mockResolvedValue(null);
    // A timeline for a branch that does not exist fails; that is not news.
    getBranchTimelineWithRevalidation.mockImplementation(() =>
      uncached(Promise.reject(new Error('no such branch')))
    );
    const { refreshDiffRoute } = await importRefresh();

    await expect(refreshDiffRoute(opened())).resolves.toEqual({ status: 'gone', what: 'branch' });
    expect(getBranch).toHaveBeenCalledWith('b1');
  });

  it('says the review is gone for a diff that was opened for one', async () => {
    getReview.mockResolvedValue(null);
    const { refreshDiffRoute } = await importRefresh();

    await expect(refreshDiffRoute(opened({ reviewId: 'r1' }))).resolves.toEqual({
      status: 'gone',
      what: 'review',
    });
    expect(getReview).toHaveBeenCalledWith('r1');
  });

  it('says the branch, not the review, when both are gone', async () => {
    getBranch.mockResolvedValue(null);
    getReview.mockResolvedValue(null);
    const { refreshDiffRoute } = await importRefresh();

    await expect(refreshDiffRoute(opened({ reviewId: 'r1' }))).resolves.toEqual({
      status: 'gone',
      what: 'branch',
    });
  });

  it('asks about no review for a diff that was not opened for one', async () => {
    const { refreshDiffRoute } = await importRefresh();

    const result = await refreshDiffRoute(opened());

    expect(result.status).toBe('current');
    expect(getReview).not.toHaveBeenCalled();
  });

  it('keeps a diff whose review still exists', async () => {
    const { refreshDiffRoute } = await importRefresh();

    const result = await refreshDiffRoute(opened({ reviewId: 'r1' }));

    expect(result).toMatchObject({ status: 'current', route: { reviewId: 'r1' } });
  });

  it('lists the commits the branch has now, not the ones it was opened with', async () => {
    getBranchTimelineWithRevalidation.mockReturnValue(
      uncached(Promise.resolve({ commits: [commit('rebased'), commit('new')] }))
    );
    const { refreshDiffRoute } = await importRefresh();
    const route = opened();

    const result = await refreshDiffRoute(route);

    expect(getBranchTimelineWithRevalidation).toHaveBeenCalledWith('b1');
    expect(result).toEqual({
      status: 'current',
      route: { ...route, commits: [commit('rebased'), commit('new')] },
    });
  });

  it('reads the branch again rather than trust a timeline cached a while ago', async () => {
    // What the branch cards last fetched, and the read that replaces it.
    getBranchTimelineWithRevalidation.mockReturnValue({
      cached: { commits: [commit('old')] },
      fresh: Promise.resolve({ commits: [commit('old'), commit('new')] }),
    });
    const { refreshDiffRoute } = await importRefresh();

    const result = await refreshDiffRoute(opened());

    expect(result).toMatchObject({
      status: 'current',
      route: { commits: [commit('old'), commit('new')] },
    });
  });

  it('takes the cached timeline when it was fetched moments ago', async () => {
    getBranchTimelineWithRevalidation.mockReturnValue({
      cached: { commits: [commit('old'), commit('cached')] },
      fresh: null,
    });
    const { refreshDiffRoute } = await importRefresh();

    const result = await refreshDiffRoute(opened());

    expect(result).toMatchObject({
      status: 'current',
      route: { commits: [commit('old'), commit('cached')] },
    });
  });

  it('falls back to the cached timeline when the fresh read fails', async () => {
    getBranchTimelineWithRevalidation.mockImplementation(() => ({
      cached: { commits: [commit('old'), commit('cached')] },
      fresh: Promise.reject(new Error('git failed')),
    }));
    const { refreshDiffRoute } = await importRefresh();

    const result = await refreshDiffRoute(opened());

    expect(result).toMatchObject({
      status: 'current',
      route: { commits: [commit('old'), commit('cached')] },
    });
  });

  it('follows a renamed branch, a changed base and a renamed project in the labels', async () => {
    getBranch.mockResolvedValue(branch({ branchName: 'renamed', baseBranch: 'origin/release' }));
    projects = [{ id: 'p1', name: 'Renamed project' }];
    const { refreshDiffRoute } = await importRefresh();

    const result = await refreshDiffRoute(opened());

    expect(result).toMatchObject({
      status: 'current',
      route: {
        branchLabel: 'renamed',
        baseBranchLabel: 'release',
        // The branch diff's own before / after were the branch's labels.
        beforeLabel: 'release',
        afterLabel: 'renamed',
        projectName: 'Renamed project',
      },
    });
  });

  it("leaves a commit diff's own labels, and what it compares, as opened", async () => {
    getBranch.mockResolvedValue(branch({ branchName: 'renamed', baseBranch: 'origin/release' }));
    // A rebase dropped the commit from the branch.
    getBranchTimelineWithRevalidation.mockReturnValue(
      uncached(Promise.resolve({ commits: [commit('rebased')] }))
    );
    const { refreshDiffRoute } = await importRefresh();
    const route = opened({
      scope: 'commit',
      commitSha: 'abc1234def',
      beforeLabel: 'parent',
      afterLabel: 'abc1234',
      readonly: true,
    });

    const result = await refreshDiffRoute(route);

    expect(result).toEqual({
      status: 'current',
      route: {
        ...route,
        commits: [commit('rebased')],
        branchLabel: 'renamed',
        baseBranchLabel: 'release',
      },
    });
  });

  it("names the branch's project for a diff opened without one", async () => {
    getBranch.mockResolvedValue(branch({ projectId: 'p2' }));
    projects = [
      { id: 'p1', name: 'Project' },
      { id: 'p2', name: 'Other' },
    ];
    const { refreshDiffRoute } = await importRefresh();

    const result = await refreshDiffRoute(opened({ projectId: undefined, projectName: undefined }));

    expect(result).toMatchObject({ status: 'current', route: { projectName: 'Other' } });
    // The route still names no project: where the diff shows is unchanged.
    expect(result.status === 'current' && result.route.projectId).toBeUndefined();
  });

  it('keeps the project name it has when the project is not in the list', async () => {
    projects = [];
    const { refreshDiffRoute } = await importRefresh();

    const result = await refreshDiffRoute(opened());

    expect(result).toMatchObject({ status: 'current', route: { projectName: 'Project' } });
  });

  it('keeps the commits it has when the timeline cannot be read', async () => {
    getBranchTimelineWithRevalidation.mockImplementation(() =>
      uncached(Promise.reject(new Error('git failed')))
    );
    const { refreshDiffRoute } = await importRefresh();
    const route = opened();

    const result = await refreshDiffRoute(route);

    expect(result).toEqual({ status: 'current', route });
    // A new route all the same: the caller swaps the entry for it.
    expect(result.status === 'current' && result.route).not.toBe(route);
  });

  it('does not know when the branch cannot be read', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    getBranch.mockRejectedValue(new Error('backend down'));
    const { refreshDiffRoute } = await importRefresh();

    await expect(refreshDiffRoute(opened())).resolves.toEqual({ status: 'unknown' });
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('does not know when the review cannot be read', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    getReview.mockRejectedValue(new Error('backend down'));
    const { refreshDiffRoute } = await importRefresh();

    await expect(refreshDiffRoute(opened({ reviewId: 'r1' }))).resolves.toEqual({
      status: 'unknown',
    });
  });

  it('does not hold the step for a branch read that never answers', async () => {
    vi.useFakeTimers();
    getBranch.mockReturnValue(new Promise(() => {}));
    const { refreshDiffRoute, DIFF_REFRESH_TIMEOUT_MS } = await importRefresh();

    const refreshing = refreshDiffRoute(opened());
    let settled = false;
    void refreshing.then(() => (settled = true));
    await vi.advanceTimersByTimeAsync(DIFF_REFRESH_TIMEOUT_MS - 1);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);

    await expect(refreshing).resolves.toEqual({ status: 'unknown' });
  });

  it('shows the diff with the commits it has when only the timeline is slow', async () => {
    vi.useFakeTimers();
    getBranch.mockResolvedValue(branch({ branchName: 'renamed' }));
    getBranchTimelineWithRevalidation.mockReturnValue(uncached(new Promise(() => {})));
    const { refreshDiffRoute, DIFF_REFRESH_TIMEOUT_MS } = await importRefresh();
    const route = opened();

    const refreshing = refreshDiffRoute(route);
    await vi.advanceTimersByTimeAsync(DIFF_REFRESH_TIMEOUT_MS);

    // The branch was read, so the labels follow it; the list is the old one.
    await expect(refreshing).resolves.toEqual({
      status: 'current',
      route: { ...route, branchLabel: 'renamed', afterLabel: 'renamed' },
    });
  });

  it('leaves no timer behind once it has answered', async () => {
    vi.useFakeTimers();
    const { refreshDiffRoute } = await importRefresh();

    await refreshDiffRoute(opened());

    expect(vi.getTimerCount()).toBe(0);
  });
});
