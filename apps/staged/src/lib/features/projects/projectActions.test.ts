import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, Project } from '../../types';

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

// ── Mock plumbing ──

let deleteProject: ReturnType<typeof vi.fn>;
let hasUnpushedCommits: ReturnType<typeof vi.fn>;
let invalidateProjectBranchTimelines: ReturnType<typeof vi.fn>;
let markAsRead: ReturnType<typeof vi.fn>;
let markAsUnread: ReturnType<typeof vi.fn>;
let clearBranchState: ReturnType<typeof vi.fn>;
let toastError: ReturnType<typeof vi.fn>;
let selectProject: ReturnType<typeof vi.fn>;
let goHome: ReturnType<typeof vi.fn>;
let navigationState: { selectedProjectId: string | null };
let projectDeleteStarted: ReturnType<typeof vi.fn>;
let projectDeleteFinished: ReturnType<typeof vi.fn>;

/** Mutable backing state for the mocked projectsDataStore. */
let storeState: {
  projects: Project[];
  branchesByProject: Map<string, Branch[]>;
  repoCountsByProject: Map<string, number>;
  deletingProjectNames: Map<string, string>;
};

async function importActions() {
  const { projectActions } = await import('./projectActions.svelte');
  return projectActions;
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (projectsData.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);

  deleteProject = vi.fn().mockResolvedValue(undefined);
  hasUnpushedCommits = vi.fn().mockResolvedValue(false);
  invalidateProjectBranchTimelines = vi.fn();
  markAsRead = vi.fn();
  markAsUnread = vi.fn();
  clearBranchState = vi.fn();
  toastError = vi.fn();
  selectProject = vi.fn();
  goHome = vi.fn();
  navigationState = { selectedProjectId: null };
  projectDeleteStarted = vi.fn();
  projectDeleteFinished = vi.fn();

  storeState = {
    projects: [project()],
    branchesByProject: new Map([['p1', [mergedBranch()]]]),
    repoCountsByProject: new Map([['p1', 1]]),
    deletingProjectNames: new Map(),
  };

  vi.doMock('../../api/commands', () => ({
    deleteProject,
    hasUnpushedCommits,
    invalidateProjectBranchTimelines,
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
      get repoCountsByProject() {
        return storeState.repoCountsByProject;
      },
      isProjectDeleting: (projectId: string) => storeState.deletingProjectNames.has(projectId),
      projectDeleteStarted,
      projectDeleteFinished,
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

describe('requestRemoveProject', () => {
  it('deletes immediately when every branch is merged with nothing unpushed', async () => {
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(actions.pendingDelete).toBeNull();
    expect(projectDeleteStarted).toHaveBeenCalledWith('p1', 'Alpha');
    expect(deleteProject).toHaveBeenCalledWith('p1');
    expect(markAsRead).toHaveBeenCalledWith('p1');
    expect(projectDeleteFinished).toHaveBeenCalledWith('p1', { removed: true });
    expect(invalidateProjectBranchTimelines).toHaveBeenCalledWith(['b1']);
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
    expect(projectDeleteFinished).toHaveBeenCalledWith('p1');
    expect(markAsRead).not.toHaveBeenCalled();
    expect(clearBranchState).not.toHaveBeenCalled();
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
    expect(projectDeleteFinished).toHaveBeenCalledWith('p1', { removed: true });
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
