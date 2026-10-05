import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, Project, ProjectRepo } from '../../types';

// ── Fixtures ──

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: 'p1',
    name: 'Alpha',
    githubRepo: 'org/alpha',
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
    statusOverride: null,
    ...overrides,
  };
}

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'b1',
    projectId: 'p1',
    projectRepoId: 'r1',
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: '/wt/b1',
    createdAt: 0,
    updatedAt: 0,
    prState: null,
    prChecksStatus: null,
    prReviewDecision: null,
    prMergeable: null,
    prDraft: null,
    prUrl: null,
    prUpdatedAt: null,
    prFetchedAt: null,
    prHeadSha: null,
    ...overrides,
  };
}

/** A branch that passes canDeleteProjectWithoutConfirmation. */
function mergedBranch(overrides: Partial<Branch> = {}): Branch {
  return branch({ prState: 'MERGED', ...overrides });
}

function projectRepo(overrides: Partial<ProjectRepo> = {}): ProjectRepo {
  return {
    id: 'r1',
    projectId: 'p1',
    githubRepo: 'org/alpha',
    branchName: 'feature',
    subpath: null,
    isPrimary: true,
    reason: null,
    headRepo: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

// ── Mock plumbing ──

let deleteProject: ReturnType<typeof vi.fn>;
let hasUnpushedCommits: ReturnType<typeof vi.fn>;
let markAsRead: ReturnType<typeof vi.fn>;
let markAsUnread: ReturnType<typeof vi.fn>;
let clearBranchState: ReturnType<typeof vi.fn>;
let toastError: ReturnType<typeof vi.fn>;
let selectProject: ReturnType<typeof vi.fn>;
let goHome: ReturnType<typeof vi.fn>;
let navigationState: { selectedProjectId: string | null };
let projectDeleteStarted: ReturnType<typeof vi.fn>;
let projectDeleteFailed: ReturnType<typeof vi.fn>;
let ensureProjectHydrated: ReturnType<typeof vi.fn>;
let setProjectStatusOverride: ReturnType<typeof vi.fn>;
let projectStatusOverrideChanged: ReturnType<typeof vi.fn>;

/** Mutable backing state for the mocked projectsDataStore. */
let storeState: {
  projects: Project[];
  branchesByProject: Map<string, Branch[]>;
  reposByProject: Map<string, ProjectRepo[]>;
  deletingProjectNames: Map<string, string>;
};

async function importActions() {
  const { projectActions } = await import('./projectActions.svelte');
  return projectActions;
}

function deferredWrite() {
  let resolve!: () => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<void>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (projectsData.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);

  deleteProject = vi.fn().mockResolvedValue(undefined);
  hasUnpushedCommits = vi.fn().mockResolvedValue(false);
  markAsRead = vi.fn();
  markAsUnread = vi.fn();
  clearBranchState = vi.fn();
  toastError = vi.fn();
  selectProject = vi.fn();
  goHome = vi.fn();
  navigationState = { selectedProjectId: null };
  projectDeleteStarted = vi.fn();
  projectDeleteFailed = vi.fn();
  // Default: the project is already hydrated, so ensuring it is a no-op.
  ensureProjectHydrated = vi.fn().mockResolvedValue(undefined);
  setProjectStatusOverride = vi.fn().mockResolvedValue(undefined);
  projectStatusOverrideChanged = vi.fn((projectId: string, statusOverride: string | null) => {
    storeState.projects = storeState.projects.map((p) =>
      p.id === projectId ? { ...p, statusOverride } : p
    );
  });

  storeState = {
    projects: [project()],
    branchesByProject: new Map([['p1', [mergedBranch()]]]),
    reposByProject: new Map([['p1', [projectRepo()]]]),
    deletingProjectNames: new Map(),
  };

  vi.doMock('../../api/commands', () => ({
    deleteProject,
    hasUnpushedCommits,
    setProjectStatusOverride,
  }));
  vi.doMock('../layout/navigation.svelte', () => ({
    navigation: navigationState,
    selectProject,
    goHome,
  }));
  vi.doMock('../../stores/projectsData.svelte', () => ({
    projectsDataStore: {
      get projects() {
        return storeState.projects;
      },
      get branchesByProject() {
        return storeState.branchesByProject;
      },
      get reposByProject() {
        return storeState.reposByProject;
      },
      // Derived exactly like the real store's, fallbacks included, so the
      // un-hydrated cases here see what the app sees.
      get repoCountsByProject() {
        return new Map(
          storeState.projects.map((p) => {
            const repos = storeState.reposByProject.get(p.id);
            return [p.id, repos ? repos.length : p.githubRepo ? 1 : 0] as const;
          })
        );
      },
      isProjectDeleting: (projectId: string) => storeState.deletingProjectNames.has(projectId),
      ensureProjectHydrated,
      projectDeleteStarted,
      projectDeleteFailed,
      projectStatusOverrideChanged,
    },
  }));
  vi.doMock('../../stores/projectState.svelte', () => ({
    projectStateStore: { markAsRead, markAsUnread },
  }));
  vi.doMock('./workspaceLifecycle.svelte', () => ({
    workspaceLifecycle: { clearBranchState },
  }));
  vi.doMock('../../shared/utils', () => ({
    projectDisplayName: (p: Project) => p.name ?? p.githubRepo ?? p.id,
  }));
  vi.doMock('svelte-sonner', () => ({
    toast: { error: toastError },
  }));
});

afterEach(() => {
  vi.doUnmock('../../api/commands');
  vi.doUnmock('../layout/navigation.svelte');
  vi.doUnmock('../../stores/projectsData.svelte');
  vi.doUnmock('../../stores/projectState.svelte');
  vi.doUnmock('./workspaceLifecycle.svelte');
  vi.doUnmock('../../shared/utils');
  vi.doUnmock('svelte-sonner');
  vi.unstubAllGlobals();
});

// ── Tests ──

describe('markProjectUnread', () => {
  it('marks the project unread', async () => {
    const actions = await importActions();
    actions.markProjectUnread(project());
    expect(markAsUnread).toHaveBeenCalledWith('p1');
  });

  it('ignores projects that are being deleted', async () => {
    storeState.deletingProjectNames.set('p1', 'Alpha');
    const actions = await importActions();
    actions.markProjectUnread(project());
    expect(markAsUnread).not.toHaveBeenCalled();
  });
});

describe('setProjectStatusOverride', () => {
  it('patches the store before writing the chosen status', async () => {
    const actions = await importActions();
    await actions.setProjectStatusOverride(project(), 'blocked');
    expect(projectStatusOverrideChanged).toHaveBeenCalledWith('p1', 'blocked');
    expect(setProjectStatusOverride).toHaveBeenCalledWith('p1', 'blocked');
    expect(projectStatusOverrideChanged.mock.invocationCallOrder[0]).toBeLessThan(
      setProjectStatusOverride.mock.invocationCallOrder[0]
    );
  });

  it('clears the override when going back to Default', async () => {
    const actions = await importActions();
    await actions.setProjectStatusOverride(project({ statusOverride: 'done' }), null);
    expect(projectStatusOverrideChanged).toHaveBeenCalledWith('p1', null);
    expect(setProjectStatusOverride).toHaveBeenCalledWith('p1', null);
  });

  it('no-ops when the status is unchanged', async () => {
    const actions = await importActions();
    await actions.setProjectStatusOverride(project({ statusOverride: 'done' }), 'done');
    await actions.setProjectStatusOverride(project(), null);
    expect(projectStatusOverrideChanged).not.toHaveBeenCalled();
    expect(setProjectStatusOverride).not.toHaveBeenCalled();
  });

  it('ignores projects that are being deleted', async () => {
    storeState.deletingProjectNames.set('p1', 'Alpha');
    const actions = await importActions();
    await actions.setProjectStatusOverride(project(), 'blocked');
    expect(projectStatusOverrideChanged).not.toHaveBeenCalled();
    expect(setProjectStatusOverride).not.toHaveBeenCalled();
  });

  it('rolls back and surfaces a failed write', async () => {
    setProjectStatusOverride.mockRejectedValueOnce(new Error('db locked'));
    const actions = await importActions();
    await actions.setProjectStatusOverride(project({ statusOverride: 'done' }), 'blocked');
    expect(projectStatusOverrideChanged.mock.calls).toEqual([
      ['p1', 'blocked'],
      ['p1', 'done'],
    ]);
    expect(toastError).toHaveBeenCalledWith('Unable to set project status', {
      description: 'db locked',
    });
  });

  it.each(['pending', 'confirmed'])(
    'preserves a newer %s selection when an older write fails',
    async (state) => {
      const blocked = deferredWrite();
      const done = deferredWrite();
      setProjectStatusOverride
        .mockReturnValueOnce(blocked.promise)
        .mockReturnValueOnce(done.promise);
      const actions = await importActions();

      const first = actions.setProjectStatusOverride(storeState.projects[0], 'blocked');
      const second = actions.setProjectStatusOverride(storeState.projects[0], 'done');
      if (state === 'confirmed') {
        done.resolve();
        await second;
        // The successful write's project-changed refetch replaces the project.
        storeState.projects = [project({ statusOverride: 'done', updatedAt: 1 })];
      }

      blocked.reject(new Error('older write failed'));
      await first;
      expect(storeState.projects[0].statusOverride).toBe('done');
      expect(projectStatusOverrideChanged.mock.calls).toEqual([
        ['p1', 'blocked'],
        ['p1', 'done'],
      ]);

      done.resolve();
      await second;
      expect(storeState.projects[0].statusOverride).toBe('done');
    }
  );

  it('still rolls back the latest selection after an older write succeeds', async () => {
    const blocked = deferredWrite();
    const done = deferredWrite();
    setProjectStatusOverride.mockReturnValueOnce(blocked.promise).mockReturnValueOnce(done.promise);
    const actions = await importActions();

    const first = actions.setProjectStatusOverride(storeState.projects[0], 'blocked');
    const second = actions.setProjectStatusOverride(storeState.projects[0], 'done');
    blocked.resolve();
    await first;
    done.reject(new Error('latest write failed'));
    await second;

    expect(storeState.projects[0].statusOverride).toBe('blocked');
    expect(toastError).toHaveBeenCalledWith('Unable to set project status', {
      description: 'latest write failed',
    });
  });

  it('distinguishes a repeated selection from an older pending write of the same status', async () => {
    const olderBlocked = deferredWrite();
    const newerBlocked = deferredWrite();
    setProjectStatusOverride
      .mockReturnValueOnce(olderBlocked.promise)
      .mockResolvedValueOnce(undefined)
      .mockReturnValueOnce(newerBlocked.promise);
    const actions = await importActions();

    const first = actions.setProjectStatusOverride(storeState.projects[0], 'blocked');
    await actions.setProjectStatusOverride(storeState.projects[0], 'done');
    const third = actions.setProjectStatusOverride(storeState.projects[0], 'blocked');
    olderBlocked.reject(new Error('older write failed'));
    await first;
    expect(storeState.projects[0].statusOverride).toBe('blocked');

    newerBlocked.reject(new Error('latest write failed'));
    await third;
    expect(storeState.projects[0].statusOverride).toBe('done');
  });

  it('tracks pending selections independently for each project', async () => {
    const firstWrite = deferredWrite();
    const secondWrite = deferredWrite();
    setProjectStatusOverride
      .mockReturnValueOnce(firstWrite.promise)
      .mockReturnValueOnce(secondWrite.promise);
    storeState.projects = [project(), project({ id: 'p2', statusOverride: 'done' })];
    const actions = await importActions();

    const first = actions.setProjectStatusOverride(storeState.projects[0], 'blocked');
    const second = actions.setProjectStatusOverride(storeState.projects[1], 'blocked');
    firstWrite.reject(new Error('first project write failed'));
    await first;
    expect(storeState.projects.map((p) => p.statusOverride)).toEqual([null, 'blocked']);

    secondWrite.reject(new Error('second project write failed'));
    await second;
    expect(storeState.projects.map((p) => p.statusOverride)).toEqual([null, 'done']);
  });
});

describe('requestRemoveProject', () => {
  it('deletes immediately when every branch is merged with nothing unpushed', async () => {
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(actions.pendingDelete).toBeNull();
    expect(projectDeleteStarted).toHaveBeenCalledWith('p1', 'Alpha');
    expect(deleteProject).toHaveBeenCalledWith('p1');
    expect(markAsRead).toHaveBeenCalledWith('p1');
    // Success leaves the deleting marker alone: the delete's project-changed
    // refetch prunes the project and the marker together.
    expect(projectDeleteFailed).not.toHaveBeenCalled();
    expect(clearBranchState).toHaveBeenCalledWith('b1');
  });

  it('asks for confirmation when a branch has unpushed work', async () => {
    hasUnpushedCommits.mockResolvedValue(true);
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(actions.pendingDelete).toEqual(project());
    expect(deleteProject).not.toHaveBeenCalled();
    expect(projectDeleteStarted).not.toHaveBeenCalled();
  });

  it('no-ops when the project is already being deleted', async () => {
    storeState.deletingProjectNames.set('p1', 'Alpha');
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(actions.pendingDelete).toBeNull();
    expect(deleteProject).not.toHaveBeenCalled();
  });

  it('surfaces a failed delete and finishes without removing', async () => {
    deleteProject.mockRejectedValue(new Error('backend down'));
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(toastError).toHaveBeenCalledWith('Unable to delete project', {
      description: 'backend down',
    });
    expect(projectDeleteFailed).toHaveBeenCalledWith('p1');
    expect(markAsRead).not.toHaveBeenCalled();
    expect(clearBranchState).not.toHaveBeenCalled();
  });
});

describe('hydration before the safety check', () => {
  /** An un-hydrated project: repos never fetched, branches seeded empty. With
   *  githubRepo null the repoCount fallback is 0, which reads as safe. */
  function unhydratedMultiRepoProject(): Project {
    const p = project({ githubRepo: null });
    storeState.projects = [p];
    storeState.branchesByProject = new Map([['p1', []]]);
    storeState.reposByProject = new Map();
    return p;
  }

  /** Make ensureProjectHydrated land real branches and repos. */
  function hydrationReveals(branches: Branch[], repos: ProjectRepo[]): void {
    ensureProjectHydrated.mockImplementation(async (projectId: string) => {
      storeState.branchesByProject.set(projectId, branches);
      storeState.reposByProject.set(projectId, repos);
    });
  }

  it('confirms instead of deleting when hydration reveals unpushed work', async () => {
    const p = unhydratedMultiRepoProject();
    hydrationReveals([mergedBranch()], [projectRepo()]);
    hasUnpushedCommits.mockResolvedValue(true);
    const actions = await importActions();

    await actions.requestRemoveProject(p);

    expect(ensureProjectHydrated).toHaveBeenCalledWith('p1');
    expect(actions.pendingDelete).toEqual(p);
    expect(deleteProject).not.toHaveBeenCalled();
    expect(projectDeleteStarted).not.toHaveBeenCalled();
  });

  it('confirms when hydration settled without fetching repos (failed load)', async () => {
    // hydrateOnce marks a project settled even when its fetch rejects, so
    // "hydrated" alone proves nothing — an unwritten repos entry does.
    const p = unhydratedMultiRepoProject();
    const actions = await importActions();

    await actions.requestRemoveProject(p);

    expect(actions.pendingDelete).toEqual(p);
    expect(deleteProject).not.toHaveBeenCalled();
    expect(hasUnpushedCommits).not.toHaveBeenCalled();
  });

  it('still deletes a fetched, repo-less project immediately', async () => {
    const p = unhydratedMultiRepoProject();
    hydrationReveals([], []);
    const actions = await importActions();

    await actions.requestRemoveProject(p);

    expect(actions.pendingDelete).toBeNull();
    expect(deleteProject).toHaveBeenCalledWith('p1');
  });

  it('clears the branches hydration revealed, not the seeded empty list', async () => {
    const p = unhydratedMultiRepoProject();
    hydrationReveals([mergedBranch(), mergedBranch({ id: 'b2' })], [projectRepo()]);
    const actions = await importActions();

    await actions.requestRemoveProject(p);

    expect(deleteProject).toHaveBeenCalledWith('p1');
    expect(clearBranchState).toHaveBeenCalledWith('b1');
    expect(clearBranchState).toHaveBeenCalledWith('b2');
  });

  it('bails when a delete for the project started while it hydrated', async () => {
    const p = unhydratedMultiRepoProject();
    ensureProjectHydrated.mockImplementation(async (projectId: string) => {
      storeState.branchesByProject.set(projectId, [mergedBranch()]);
      storeState.reposByProject.set(projectId, [projectRepo()]);
      storeState.deletingProjectNames.set(projectId, 'Alpha');
    });
    hasUnpushedCommits.mockResolvedValue(true);
    const actions = await importActions();

    await actions.requestRemoveProject(p);

    expect(actions.pendingDelete).toBeNull();
    expect(deleteProject).not.toHaveBeenCalled();
    expect(projectDeleteStarted).not.toHaveBeenCalled();
  });
});

describe('confirmation dialog flow', () => {
  it('confirmPendingDelete deletes the pending project', async () => {
    hasUnpushedCommits.mockResolvedValue(true);
    const actions = await importActions();
    await actions.requestRemoveProject(project());
    expect(actions.pendingDelete).not.toBeNull();

    await actions.confirmPendingDelete();

    expect(actions.pendingDelete).toBeNull();
    expect(deleteProject).toHaveBeenCalledWith('p1');
    expect(projectDeleteFailed).not.toHaveBeenCalled();
  });

  it('cancelPendingDelete dismisses without deleting', async () => {
    hasUnpushedCommits.mockResolvedValue(true);
    const actions = await importActions();
    await actions.requestRemoveProject(project());

    actions.cancelPendingDelete();

    expect(actions.pendingDelete).toBeNull();
    expect(deleteProject).not.toHaveBeenCalled();
  });

  it('confirmPendingDelete is a no-op with nothing pending', async () => {
    const actions = await importActions();
    await actions.confirmPendingDelete();
    expect(deleteProject).not.toHaveBeenCalled();
  });
});

describe('navigation on delete', () => {
  it('selects the next alive project when deleting the selected one', async () => {
    const p2 = project({ id: 'p2', name: 'Beta' });
    storeState.projects = [project(), p2];
    storeState.branchesByProject.set('p2', []);
    navigationState.selectedProjectId = 'p1';
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(selectProject).toHaveBeenCalledWith('p2');
    expect(goHome).not.toHaveBeenCalled();
  });

  it('falls back to the closest earlier project when nothing follows', async () => {
    const p2 = project({ id: 'p2', name: 'Beta' });
    storeState.projects = [p2, project()];
    storeState.branchesByProject.set('p2', []);
    navigationState.selectedProjectId = 'p1';
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(selectProject).toHaveBeenCalledWith('p2');
  });

  it('goes home when the selected project was the last one', async () => {
    navigationState.selectedProjectId = 'p1';
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(goHome).toHaveBeenCalled();
    expect(selectProject).not.toHaveBeenCalled();
  });

  it('stays put when deleting a project that is not selected', async () => {
    const p2 = project({ id: 'p2', name: 'Beta' });
    storeState.projects = [project(), p2];
    storeState.branchesByProject.set('p2', []);
    navigationState.selectedProjectId = 'p2';
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(selectProject).not.toHaveBeenCalled();
    expect(goHome).not.toHaveBeenCalled();
    expect(deleteProject).toHaveBeenCalledWith('p1');
  });

  it('stays put on the repos route (no project selected)', async () => {
    navigationState.selectedProjectId = null;
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(selectProject).not.toHaveBeenCalled();
    expect(goHome).not.toHaveBeenCalled();
    expect(deleteProject).toHaveBeenCalledWith('p1');
  });
});
