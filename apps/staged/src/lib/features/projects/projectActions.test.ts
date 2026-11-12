import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  Branch,
  Project,
  ProjectRepo,
  ProjectTransferPreflight,
  ProjectTransferPreflightEvent,
  ProjectTransferRepoPreflight,
  ProjectTransferResult,
} from '../../types';

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
    prTitle: null,
    deferredPush: null,
    deferredPushRequestedAt: null,
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

function repoPreflight(
  overrides: Partial<ProjectTransferRepoPreflight> = {}
): ProjectTransferRepoPreflight {
  return {
    branchId: 'b1',
    projectRepoId: 'r1',
    githubRepo: 'org/alpha',
    subpath: null,
    branchName: 'feature',
    destBranchName: 'feature',
    state: 'carry',
    head: 'abc',
    upstream: 'origin/feature',
    unpushedCommits: 0,
    staged: 0,
    unstaged: 0,
    untracked: 0,
    conflicted: 0,
    untrackedBytes: 0,
    ignoredFiles: 0,
    ignoredBytes: 0,
    ignoredDirs: 0,
    lfsPresent: 0,
    lfsPresentBytes: 0,
    lfsMissing: 0,
    lfsMissingBytes: 0,
    hasUncommittedChanges: false,
    scanning: false,
    blockers: [],
    warnings: [],
    ...overrides,
  };
}

function preflight(overrides: Partial<ProjectTransferPreflight> = {}): ProjectTransferPreflight {
  return {
    projectId: 'p1',
    from: 'local',
    to: 'remote',
    direction: 'toRemote',
    workspaceName: null,
    repos: [repoPreflight()],
    blockers: [],
    warnings: [],
    canTransfer: true,
    hasUncommittedChanges: false,
    scanning: false,
    ...overrides,
  };
}

function transferResult(overrides: Partial<ProjectTransferResult> = {}): ProjectTransferResult {
  return {
    projectId: 'p1',
    transferId: 't1',
    direction: 'toRemote',
    location: 'remote',
    workspaceName: 'alpha-feature',
    repos: [
      {
        branchId: 'b1',
        projectRepoId: 'r1',
        githubRepo: 'org/alpha',
        branchName: 'feature',
        previousBranchName: 'feature',
        carried: true,
        head: 'abc',
        uncommittedRestored: false,
        uncommittedLeftBehind: false,
        retriedWithoutExclusions: false,
        backupRef: 'refs/staged-transfer-backup/t1/feature',
        warnings: [],
      },
    ],
    warnings: [],
    backupDir: '/home/me/.staged/transfers/t1',
    ...overrides,
  };
}

// ── Mock plumbing ──

let deleteProject: ReturnType<typeof vi.fn>;
let startProjectTransferPreflight: ReturnType<typeof vi.fn>;
let cancelProjectTransferPreflight: ReturnType<typeof vi.fn>;
let transferProject: ReturnType<typeof vi.fn>;
let cancelProjectTransfer: ReturnType<typeof vi.fn>;
let projectTransferStarted: ReturnType<typeof vi.fn>;
let projectTransferFinished: ReturnType<typeof vi.fn>;
let refreshTransferredProject: ReturnType<typeof vi.fn>;
let setTransferUncommittedChanges: ReturnType<typeof vi.fn>;
let setTransferIgnoredFiles: ReturnType<typeof vi.fn>;
let toastSuccess: ReturnType<typeof vi.fn>;
let toastWarning: ReturnType<typeof vi.fn>;
let toastInfo: ReturnType<typeof vi.fn>;
let hasUnpushedCommits: ReturnType<typeof vi.fn>;
let markAsRead: ReturnType<typeof vi.fn>;
let markAsUnread: ReturnType<typeof vi.fn>;
let clearBranchState: ReturnType<typeof vi.fn>;
let toastError: ReturnType<typeof vi.fn>;
let selectProject: ReturnType<typeof vi.fn>;
let goHome: ReturnType<typeof vi.fn>;
let releaseProjectLayer: ReturnType<typeof vi.fn>;
let navigationState: {
  selectedProjectId: string | null;
  homeActive: boolean;
  projectLayer: { kind: 'project'; projectId: string } | null;
};
let projectDeleteStarted: ReturnType<typeof vi.fn>;
let projectDeleteFailed: ReturnType<typeof vi.fn>;
let ensureProjectHydrated: ReturnType<typeof vi.fn>;

/** Mutable backing state for the mocked projectsDataStore. */
let storeState: {
  projects: Project[];
  branchesByProject: Map<string, Branch[]>;
  reposByProject: Map<string, ProjectRepo[]>;
  deletingProjectNames: Map<string, string>;
  transferringProjects: Map<string, string>;
};

async function importActions() {
  const { projectActions } = await import('./projectActions.svelte');
  return projectActions;
}

// ── The streamed preflight scan ──
//
// requestTransferProject listens for `project-transfer-preflight`, then
// starts a scan under a fresh id; the backend answers with partial preflights
// and one final `done` event. The fake here hands the listener's callback to
// the test, records the scan id, and by default finishes the scan with
// `scanResult` on the next macrotask.

let scanListener: ((event: ProjectTransferPreflightEvent) => void) | null;
let scanUnlisten: ReturnType<typeof vi.fn>;
let lastScanId: string | null;
/** How a started scan ends by itself; `hold` leaves it to the test. */
let scanResult: { preflight: ProjectTransferPreflight } | { error: string } | 'hold';

function scanEvent(
  overrides: Partial<ProjectTransferPreflightEvent>
): ProjectTransferPreflightEvent {
  return {
    projectId: 'p1',
    scanId: lastScanId ?? 'none',
    preflight: null,
    done: false,
    cancelled: false,
    error: null,
    ...overrides,
  };
}

function emitScan(overrides: Partial<ProjectTransferPreflightEvent>): void {
  scanListener?.(scanEvent(overrides));
}

function scanResolves(pf: ProjectTransferPreflight): void {
  scanResult = { preflight: pf };
}

/** Let the delete's post-flush focus check run. */
function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (agent.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);

  deleteProject = vi.fn().mockResolvedValue(undefined);
  scanListener = null;
  lastScanId = null;
  scanResult = { preflight: preflight() };
  scanUnlisten = vi.fn(() => {
    scanListener = null;
  });
  startProjectTransferPreflight = vi.fn(async (_projectId: string, scanId: string) => {
    lastScanId = scanId;
    const result = scanResult;
    if (result === 'hold') return;
    setTimeout(() => {
      if ('error' in result) emitScan({ scanId, done: true, error: result.error });
      else emitScan({ scanId, done: true, preflight: result.preflight });
    }, 0);
  });
  cancelProjectTransferPreflight = vi.fn(async () => true);
  transferProject = vi.fn().mockResolvedValue(transferResult());
  cancelProjectTransfer = vi.fn().mockResolvedValue(true);
  projectTransferStarted = vi.fn((projectId: string, direction: string) => {
    storeState.transferringProjects.set(projectId, direction);
  });
  projectTransferFinished = vi.fn((projectId: string) => {
    storeState.transferringProjects.delete(projectId);
  });
  refreshTransferredProject = vi.fn().mockResolvedValue(undefined);
  setTransferUncommittedChanges = vi.fn();
  setTransferIgnoredFiles = vi.fn();
  toastSuccess = vi.fn();
  toastWarning = vi.fn();
  toastInfo = vi.fn();
  hasUnpushedCommits = vi.fn().mockResolvedValue(false);
  markAsRead = vi.fn();
  markAsUnread = vi.fn();
  clearBranchState = vi.fn();
  toastError = vi.fn();
  selectProject = vi.fn();
  goHome = vi.fn();
  releaseProjectLayer = vi.fn();
  navigationState = { selectedProjectId: null, homeActive: true, projectLayer: null };
  projectDeleteStarted = vi.fn();
  projectDeleteFailed = vi.fn();
  // Default: the project is already hydrated, so ensuring it is a no-op.
  ensureProjectHydrated = vi.fn().mockResolvedValue(undefined);

  storeState = {
    projects: [project()],
    branchesByProject: new Map([['p1', [mergedBranch()]]]),
    reposByProject: new Map([['p1', [projectRepo()]]]),
    deletingProjectNames: new Map(),
    transferringProjects: new Map(),
  };

  vi.doMock('../../api/commands', () => ({
    deleteProject,
    hasUnpushedCommits,
    startProjectTransferPreflight,
    cancelProjectTransferPreflight,
    transferProject,
    cancelProjectTransfer,
  }));
  vi.doMock('../../transport', () => ({
    listenToEvent: vi.fn(
      (
        _event: string,
        callback: (event: ProjectTransferPreflightEvent) => void,
        opts?: { onEstablished?: () => void }
      ) => {
        scanListener = callback;
        // The real transport confirms the registration asynchronously.
        setTimeout(() => opts?.onEstablished?.(), 0);
        return scanUnlisten;
      }
    ),
  }));
  vi.doMock('../settings/preferences.svelte', () => ({
    setTransferUncommittedChanges,
    setTransferIgnoredFiles,
  }));
  vi.doMock('../layout/navigation.svelte', () => ({
    navigation: navigationState,
    selectProject,
    goHome,
    releaseProjectLayer,
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
      isProjectTransferring: (projectId: string) => storeState.transferringProjects.has(projectId),
      ensureProjectHydrated,
      projectDeleteStarted,
      projectDeleteFailed,
      projectTransferStarted,
      projectTransferFinished,
      refreshTransferredProject,
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
    toast: { error: toastError, success: toastSuccess, warning: toastWarning, info: toastInfo },
  }));
});

afterEach(() => {
  vi.doUnmock('../../api/commands');
  vi.doUnmock('../../transport');
  vi.doUnmock('../settings/preferences.svelte');
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
    // Success leaves the deleting marker alone: the delete's project-changed
    // refetch prunes the project and the marker together.
    expect(projectDeleteFailed).not.toHaveBeenCalled();
    expect(clearBranchState).toHaveBeenCalledWith('b1');
    // The card left without a word at confirm; the toast says it went through.
    expect(toastSuccess).toHaveBeenCalledWith('Removed "Alpha"');
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
    expect(toastSuccess).not.toHaveBeenCalled();
  });
});

describe('focus after remove', () => {
  // The node environment has no DOM: just enough document for
  // focusIfDropped to read where focus is.
  const body = { tagName: 'BODY' };
  let documentState: { activeElement: unknown; body: unknown };
  let neighbour: { focus: ReturnType<typeof vi.fn> };

  beforeEach(() => {
    documentState = { activeElement: body, body };
    vi.stubGlobal('document', documentState);
    neighbour = { focus: vi.fn() };
  });

  const focusAfterRemove = () => neighbour as unknown as HTMLElement;

  it("moves focus the hidden card dropped on to the list's pick", async () => {
    const actions = await importActions();

    await actions.requestRemoveProject(project(), focusAfterRemove);
    await tick();

    expect(neighbour.focus).toHaveBeenCalledWith({ preventScroll: true });
  });

  it('leaves focus the user has put somewhere else', async () => {
    documentState.activeElement = { tagName: 'INPUT' };
    const actions = await importActions();

    await actions.requestRemoveProject(project(), focusAfterRemove);
    await tick();

    expect(neighbour.focus).not.toHaveBeenCalled();
  });

  it("hands a confirmed remove's pick to the dialog's close, once", async () => {
    hasUnpushedCommits.mockResolvedValue(true);
    const actions = await importActions();
    await actions.requestRemoveProject(project(), focusAfterRemove);
    // The Remove button unmounts with the dialog's footer in the flush that
    // hides the card. bits-ui's FocusScope then focuses the dialog's content,
    // before the delete's own focus check runs.
    documentState.activeElement = { tagName: 'DIV' };

    await actions.confirmPendingDelete();
    await tick();

    expect(neighbour.focus).not.toHaveBeenCalled();
    expect(actions.takeFocusAfterRemove()).toBe(neighbour);
    expect(actions.takeFocusAfterRemove()).toBeNull();
  });

  describe('when the removed project is the one open', () => {
    // The sidebar's repo filter shows [A1, A2, A3] over [A1, B1, A2, B2, A3]
    // with A2 open. Removing A2 opens B2, the next live project in the
    // unfiltered list, and the sidebar appends B2's row as the selected one.
    // Focus has to land there, on the row the sidebar highlights, rather
    // than on A3, the filtered list's neighbour.
    async function removeOpenProjectFromFilteredSidebar() {
      storeState.projects = ['A1', 'B1', 'A2', 'B2', 'A3'].map((id) => project({ id, name: id }));
      storeState.branchesByProject = new Map([['A2', [mergedBranch({ projectId: 'A2' })]]]);
      storeState.reposByProject = new Map([['A2', [projectRepo({ projectId: 'A2' })]]]);
      navigationState.selectedProjectId = 'A2';
      type Row = { isConnected: boolean; focus: ReturnType<typeof vi.fn> };
      const row = (): Row => ({ isConnected: true, focus: vi.fn() });
      const rows = new Map<string, Row>([
        ['A1', row()],
        ['A3', row()],
      ]);
      selectProject.mockImplementation((projectId: string) => rows.set(projectId, row()));
      const { neighbourAfterRemove } = await import('./focusAfterRemove');
      const focusAfterRemove = neighbourAfterRemove(
        ['A1', 'A2', 'A3'],
        'A2',
        (projectId) => rows.get(projectId) as unknown as HTMLElement | undefined
      );
      return { rows, removed: storeState.projects[2], focusAfterRemove };
    }

    it('focuses the row of the project the delete opened instead', async () => {
      const { rows, removed, focusAfterRemove } = await removeOpenProjectFromFilteredSidebar();
      const actions = await importActions();

      await actions.requestRemoveProject(removed, focusAfterRemove);
      await tick();

      expect(selectProject).toHaveBeenCalledWith('B2');
      expect(rows.get('B2')!.focus).toHaveBeenCalledWith({ preventScroll: true });
      expect(rows.get('A3')!.focus).not.toHaveBeenCalled();
    });

    it("hands the dialog's close the same row", async () => {
      hasUnpushedCommits.mockResolvedValue(true);
      const { rows, removed, focusAfterRemove } = await removeOpenProjectFromFilteredSidebar();
      const actions = await importActions();
      await actions.requestRemoveProject(removed, focusAfterRemove);
      // As above: the dialog's FocusScope has focused its content.
      documentState.activeElement = { tagName: 'DIV' };

      await actions.confirmPendingDelete();
      await tick();

      expect(selectProject).toHaveBeenCalledWith('B2');
      expect(actions.takeFocusAfterRemove()).toBe(rows.get('B2'));
    });
  });

  it('leaves a cancelled dialog to return focus itself', async () => {
    hasUnpushedCommits.mockResolvedValue(true);
    const actions = await importActions();
    await actions.requestRemoveProject(project(), focusAfterRemove);

    actions.cancelPendingDelete();

    expect(actions.takeFocusAfterRemove()).toBeNull();
    // A later confirm of another remove doesn't inherit the cancelled pick.
    await actions.requestRemoveProject(project({ id: 'p2', name: 'Beta' }));
    await actions.confirmPendingDelete();
    expect(actions.takeFocusAfterRemove()).toBeNull();
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

  it('releases the retained project layer as soon as the delete starts', async () => {
    // Deleting from the home grid: nothing is selected, but the project may
    // be the one retained under home. The release precedes the backend call.
    navigationState.selectedProjectId = null;
    let releasedBeforeDelete = false;
    deleteProject.mockImplementation(async () => {
      releasedBeforeDelete = releaseProjectLayer.mock.calls.length === 1;
    });
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(releaseProjectLayer).toHaveBeenCalledWith('p1');
    expect(releasedBeforeDelete).toBe(true);
  });

  it('releases the layer of the on-screen project after going home', async () => {
    // Deleting the last project while it is on screen: goHome() leaves the
    // layer naming the deleted project, and the real releaseProjectLayer only
    // acts once home is active, so the release has to follow the navigation.
    // The mock mirrors that guard so the end state is what the app would see.
    navigationState.selectedProjectId = 'p1';
    navigationState.homeActive = false;
    navigationState.projectLayer = { kind: 'project', projectId: 'p1' };
    goHome.mockImplementation(() => {
      navigationState.homeActive = true;
      navigationState.selectedProjectId = null;
    });
    releaseProjectLayer.mockImplementation((projectId: string) => {
      const layer = navigationState.projectLayer;
      if (!navigationState.homeActive || layer?.projectId !== projectId) return;
      navigationState.projectLayer = null;
    });
    // Starts undefined so a delete that never ran fails the null check too.
    let layerAtDelete: { kind: 'project'; projectId: string } | null | undefined;
    deleteProject.mockImplementation(async () => {
      layerAtDelete = navigationState.projectLayer;
    });
    const actions = await importActions();

    await actions.requestRemoveProject(project());

    expect(goHome.mock.invocationCallOrder[0]).toBeLessThan(
      releaseProjectLayer.mock.invocationCallOrder[0]
    );
    expect(layerAtDelete).toBeNull();
    expect(navigationState.homeActive).toBe(true);
  });
});

describe('requestTransferProject', () => {
  it('opens the dialog with the branches it knows as Checking…, then streams the scan into it', async () => {
    scanResult = 'hold';
    // The repos are fetched for the progress banner; the dialog opens from
    // the branches already here, so a cold project's hydration doesn't hold
    // it back.
    ensureProjectHydrated.mockReturnValue(new Promise(() => {}));
    const actions = await importActions();

    const request = actions.requestTransferProject(project());
    // Before the scan has said anything — before it has even been started —
    // the dialog has its rows, each being checked, and Transfer is live.
    expect(actions.pendingTransfer).toEqual({
      project: project(),
      direction: 'toRemote',
      status: 'ready',
      preflight: preflight({
        repos: [
          repoPreflight({
            state: 'nothingToCarry',
            head: null,
            upstream: null,
            scanning: true,
          }),
        ],
        scanning: true,
      }),
    });
    expect(ensureProjectHydrated).toHaveBeenCalledWith('p1');
    // The scan starts only once the listener is in place.
    expect(startProjectTransferPreflight).not.toHaveBeenCalled();
    await tick();
    expect(startProjectTransferPreflight).toHaveBeenCalledWith('p1', expect.any(String));

    // The first partial: the rows, still being read.
    const partial = preflight({ repos: [repoPreflight({ scanning: true })], scanning: true });
    emitScan({ preflight: partial });
    expect(actions.pendingTransfer).toEqual({
      project: project(),
      direction: 'toRemote',
      status: 'ready',
      preflight: partial,
    });

    // Another dialog's scan is not this one's.
    emitScan({ scanId: 'other', preflight: preflight({ canTransfer: false }), done: true });
    expect(
      actions.pendingTransfer?.status === 'ready' && actions.pendingTransfer.preflight
    ).toEqual(partial);

    emitScan({ preflight: preflight(), done: true });
    await request;
    expect(actions.pendingTransfer).toEqual({
      project: project(),
      direction: 'toRemote',
      status: 'ready',
      preflight: preflight(),
    });
    expect(scanUnlisten).toHaveBeenCalled();
    expect(cancelProjectTransferPreflight).not.toHaveBeenCalled();
    expect(transferProject).not.toHaveBeenCalled();
  });

  it("opens the dialog loading when the project's branches aren't known yet", async () => {
    scanResult = 'hold';
    storeState.branchesByProject = new Map();
    const actions = await importActions();
    const request = actions.requestTransferProject(project());
    expect(actions.pendingTransfer).toEqual({
      project: project(),
      direction: 'toRemote',
      status: 'loading',
    });
    await tick();
    const partial = preflight({ repos: [repoPreflight({ scanning: true })], scanning: true });
    emitScan({ preflight: partial });
    expect(actions.pendingTransfer?.status).toBe('ready');
    emitScan({ preflight: preflight(), done: true });
    await request;
  });

  it('names the workspace a remote project comes from before the scan does', async () => {
    scanResult = 'hold';
    storeState.branchesByProject = new Map([
      ['p1', [branch({ branchType: 'remote', workspaceName: 'stg-feature', worktreePath: null })]],
    ]);
    const actions = await importActions();
    const request = actions.requestTransferProject(project({ location: 'remote' }));
    const pending = actions.pendingTransfer;
    expect(pending?.status === 'ready' && pending.preflight).toMatchObject({
      direction: 'toLocal',
      from: 'remote',
      to: 'local',
      workspaceName: 'stg-feature',
      canTransfer: true,
      scanning: true,
    });
    await tick();
    emitScan({ preflight: preflight({ direction: 'toLocal' }), done: true });
    await request;
  });

  it('offers the other location for a remote project', async () => {
    scanResolves(
      preflight({ from: 'remote', to: 'local', direction: 'toLocal', workspaceName: 'ws' })
    );
    const actions = await importActions();
    await actions.requestTransferProject(project({ location: 'remote' }));
    expect(actions.pendingTransfer?.direction).toBe('toLocal');
  });

  it('shows a failed scan in the dialog', async () => {
    scanResult = { error: 'sq exploded' };
    const actions = await importActions();
    await actions.requestTransferProject(project());
    expect(actions.pendingTransfer).toEqual({
      project: project(),
      direction: 'toRemote',
      status: 'error',
      error: 'sq exploded',
    });
  });

  it('shows a scan that could not start in the dialog', async () => {
    startProjectTransferPreflight.mockRejectedValue(new Error('no store'));
    const actions = await importActions();
    await actions.requestTransferProject(project());
    expect(actions.pendingTransfer).toEqual({
      project: project(),
      direction: 'toRemote',
      status: 'error',
      error: 'no store',
    });
    expect(scanUnlisten).toHaveBeenCalled();
  });

  it('stops the scan when the dialog is closed and drops what lands after', async () => {
    scanResult = 'hold';
    const actions = await importActions();
    const request = actions.requestTransferProject(project());
    await tick();
    const scanId = lastScanId;
    actions.cancelPendingTransfer();
    expect(cancelProjectTransferPreflight).toHaveBeenCalledWith(scanId);
    expect(scanUnlisten).toHaveBeenCalled();
    // The backend's final event for a stopped scan reaches no listener; a
    // late one on a listener that somehow stayed is dropped all the same.
    scanListener = null;
    emitScan({ scanId: scanId!, preflight: preflight(), done: true, cancelled: true });
    await request;
    expect(actions.pendingTransfer).toBeNull();
  });

  it('no-ops for a project that is deleting or already transferring', async () => {
    const actions = await importActions();
    storeState.deletingProjectNames.set('p1', 'Alpha');
    await actions.requestTransferProject(project());
    storeState.deletingProjectNames.clear();
    storeState.transferringProjects.set('p1', 'toRemote');
    await actions.requestTransferProject(project());
    expect(actions.pendingTransfer).toBeNull();
    expect(startProjectTransferPreflight).not.toHaveBeenCalled();
  });
});

describe('confirmPendingTransfer', () => {
  async function readyActions(pf: ProjectTransferPreflight) {
    scanResolves(pf);
    const actions = await importActions();
    await actions.requestTransferProject(project());
    expect(actions.pendingTransfer?.status).toBe('ready');
    return actions;
  }

  it('closes the dialog, marks the project transferring and toasts the success', async () => {
    const actions = await readyActions(preflight());
    let transferringDuringCall: boolean | undefined;
    transferProject.mockImplementation(async () => {
      transferringDuringCall = storeState.transferringProjects.get('p1') === 'toRemote';
      return transferResult();
    });

    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: false });

    expect(actions.pendingTransfer).toBeNull();
    expect(projectTransferStarted).toHaveBeenCalledWith('p1', 'toRemote');
    // The direction the dialog showed, for the backend to hold it to.
    expect(transferProject).toHaveBeenCalledWith('p1', 'toRemote', true, false);
    expect(transferringDuringCall).toBe(true);
    expect(projectTransferFinished).toHaveBeenCalledWith('p1', 't1');
    expect(toastSuccess).toHaveBeenCalledWith('Transferred "Alpha" to Blox', {
      description: undefined,
    });
    expect(toastWarning).not.toHaveBeenCalled();
  });

  it('stops a scan still going and starts the transfer at once', async () => {
    scanResult = 'hold';
    const actions = await importActions();
    const request = actions.requestTransferProject(project());
    await tick();
    const scanId = lastScanId;
    emitScan({
      preflight: preflight({ repos: [repoPreflight({ scanning: true })], scanning: true }),
    });
    expect(actions.pendingTransfer?.status).toBe('ready');

    await actions.confirmPendingTransfer({ includeUncommitted: false, includeIgnored: false });

    expect(cancelProjectTransferPreflight).toHaveBeenCalledWith(scanId);
    expect(transferProject).toHaveBeenCalledWith('p1', 'toRemote', false, false);
    // A row still being read may turn out to have uncommitted changes, so the
    // checkbox was asked and its answer remembered.
    expect(setTransferUncommittedChanges).toHaveBeenCalledWith(false);
    expect(actions.pendingTransfer).toBeNull();
    await request;
    expect(actions.pendingTransfer).toBeNull();
  });

  it('remembers the checkbox only when it was shown', async () => {
    // Nothing uncommitted: no checkbox, nothing remembered, and the transfer
    // carries whatever turns up rather than applying a stale answer.
    let actions = await readyActions(preflight());
    await actions.confirmPendingTransfer({ includeUncommitted: false, includeIgnored: false });
    expect(setTransferUncommittedChanges).not.toHaveBeenCalled();
    expect(transferProject).toHaveBeenLastCalledWith('p1', 'toRemote', true, false);

    const dirty = preflight({
      repos: [repoPreflight({ unstaged: 2, hasUncommittedChanges: true })],
      hasUncommittedChanges: true,
    });
    actions = await readyActions(dirty);
    await actions.confirmPendingTransfer({ includeUncommitted: false, includeIgnored: false });
    expect(setTransferUncommittedChanges).toHaveBeenCalledWith(false);
    expect(transferProject).toHaveBeenLastCalledWith('p1', 'toRemote', false, false);

    actions = await readyActions(dirty);
    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: false });
    expect(setTransferUncommittedChanges).toHaveBeenLastCalledWith(true);
    expect(transferProject).toHaveBeenLastCalledWith('p1', 'toRemote', true, false);

    // A workspace the transfer resumes first may have changes nobody has
    // counted yet: the checkbox shows for it, and its answer applies.
    setTransferUncommittedChanges.mockClear();
    actions = await readyActions(preflight({ repos: [repoPreflight({ state: 'unknown' })] }));
    await actions.confirmPendingTransfer({ includeUncommitted: false, includeIgnored: false });
    expect(setTransferUncommittedChanges).toHaveBeenCalledWith(false);
    expect(transferProject).toHaveBeenLastCalledWith('p1', 'toRemote', false, false);
  });

  it('remembers and sends the ignored-files answer only when it could be given', async () => {
    // No ignored files: no checkbox, nothing remembered, nothing carried.
    let actions = await readyActions(preflight({ repos: [repoPreflight({ ignoredDirs: 3 })] }));
    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: true });
    expect(setTransferIgnoredFiles).not.toHaveBeenCalled();
    expect(transferProject).toHaveBeenLastCalledWith('p1', 'toRemote', true, false);

    const ignored = preflight({ repos: [repoPreflight({ ignoredFiles: 2, ignoredBytes: 40 })] });
    actions = await readyActions(ignored);
    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: true });
    expect(setTransferIgnoredFiles).toHaveBeenLastCalledWith(true);
    expect(transferProject).toHaveBeenLastCalledWith('p1', 'toRemote', true, true);

    actions = await readyActions(ignored);
    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: false });
    expect(setTransferIgnoredFiles).toHaveBeenLastCalledWith(false);
    expect(transferProject).toHaveBeenLastCalledWith('p1', 'toRemote', true, false);
    // Independent of the uncommitted-changes answer, which wasn't asked.
    expect(setTransferUncommittedChanges).not.toHaveBeenCalled();

    // Over the cap: the box is disabled, so its (remembered) value is neither
    // saved nor sent, which the backend would refuse.
    setTransferIgnoredFiles.mockClear();
    actions = await readyActions(
      preflight({
        repos: [
          repoPreflight({
            ignoredFiles: 2,
            warnings: [{ code: 'ignored_too_large', message: 'too big' }],
          }),
        ],
      })
    );
    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: true });
    expect(setTransferIgnoredFiles).not.toHaveBeenCalled();
    expect(transferProject).toHaveBeenLastCalledWith('p1', 'toRemote', true, false);
  });

  it('refuses a blocked preflight', async () => {
    const actions = await readyActions(
      preflight({
        canTransfer: false,
        blockers: [{ code: 'sq_unavailable', message: 'no sq' }],
      })
    );
    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: false });
    expect(transferProject).not.toHaveBeenCalled();
    expect(actions.pendingTransfer?.status).toBe('ready');
  });

  it('surfaces teardown and new repo warnings, not ones the dialog already showed', async () => {
    const lfs = {
      code: 'lfs_missing',
      message:
        '2 LFS objects are not available locally and will be fetched from origin if possible.',
    };
    const actions = await readyActions(preflight({ repos: [repoPreflight({ warnings: [lfs] })] }));
    const result = transferResult({
      warnings: [{ code: 'worktree_left', message: 'Transferred, but the old worktree stayed.' }],
    });
    result.repos[0].warnings = [lfs, { code: 'apply', message: 'Upstream fetch failed.' }];
    result.repos[0].uncommittedLeftBehind = true;
    transferProject.mockResolvedValue(result);

    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: false });

    expect(toastWarning).toHaveBeenCalledWith('Transferred "Alpha" to Blox', {
      description:
        'Transferred, but the old worktree stayed. Upstream fetch failed. Uncommitted changes were left behind in the backup at /home/me/.staged/transfers/t1.',
      duration: Infinity,
    });
    expect(toastSuccess).not.toHaveBeenCalled();
  });

  it('keeps the success toast open when changes were left behind', async () => {
    const actions = await readyActions(preflight());
    const result = transferResult();
    result.repos[0].uncommittedLeftBehind = true;
    transferProject.mockResolvedValue(result);

    await actions.confirmPendingTransfer({ includeUncommitted: false, includeIgnored: false });

    // It carries the backup path, the only way back to those changes.
    expect(toastSuccess).toHaveBeenCalledWith('Transferred "Alpha" to Blox', {
      description:
        'Uncommitted changes were left behind in the backup at /home/me/.staged/transfers/t1.',
      duration: Infinity,
    });
  });

  it('clears the transferring marker and toasts a failure', async () => {
    transferProject.mockRejectedValue(new Error('bundle verify failed'));
    const actions = await readyActions(preflight());

    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: false });

    expect(projectTransferFinished).toHaveBeenCalledWith('p1');
    expect(storeState.transferringProjects.has('p1')).toBe(false);
    expect(toastError).toHaveBeenCalledWith('Unable to transfer "Alpha" to Blox', {
      description: 'bundle verify failed',
      duration: Infinity,
    });
  });

  it('sends the direction the dialog of a remote project showed', async () => {
    scanResolves(
      preflight({ from: 'remote', to: 'local', direction: 'toLocal', workspaceName: 'ws' })
    );
    const actions = await importActions();
    await actions.requestTransferProject(project({ location: 'remote' }));

    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: false });

    expect(transferProject).toHaveBeenCalledWith('p1', 'toLocal', true, false);
  });

  it('shows why a dialog left open while the project moved did nothing', async () => {
    // Opened while the project was local; another window has since moved it
    // to Blox, so the backend refuses "Transfer to Blox".
    const refusal =
      'The project is already on Blox; it was transferred after this dialog opened, so nothing changed.';
    transferProject.mockRejectedValue(refusal);
    const actions = await readyActions(preflight());

    await actions.confirmPendingTransfer({ includeUncommitted: true, includeIgnored: false });

    expect(actions.pendingTransfer).toBeNull();
    expect(projectTransferFinished).toHaveBeenCalledWith('p1');
    expect(storeState.transferringProjects.has('p1')).toBe(false);
    // The backend's sentence, under a title that names no destination.
    expect(toastWarning).toHaveBeenCalledWith(`Didn't transfer "Alpha"`, {
      description: refusal,
    });
    expect(toastError).not.toHaveBeenCalled();
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(toastInfo).not.toHaveBeenCalled();
    // Where the project is now, so the next dialog offers the way it goes.
    expect(refreshTransferredProject).toHaveBeenCalledWith('p1');
  });

  it('reports a cancelled transfer as cancelled, not failed', async () => {
    transferProject.mockRejectedValue(new Error('the transfer was cancelled'));
    const actions = await readyActions(preflight());

    const confirmed = actions.confirmPendingTransfer({
      includeUncommitted: true,
      includeIgnored: false,
    });
    await expect(actions.cancelTransfer('p1')).resolves.toBe(true);
    expect(cancelProjectTransfer).toHaveBeenCalledWith('p1');
    await confirmed;

    expect(toastInfo).toHaveBeenCalledWith('Cancelled transferring "Alpha"', {
      description: 'Nothing changed; the project is still local.',
    });
    expect(toastError).not.toHaveBeenCalled();
  });

  it('blocks remove while the project is transferring', async () => {
    storeState.transferringProjects.set('p1', 'toRemote');
    const actions = await importActions();
    await actions.requestRemoveProject(project());
    expect(deleteProject).not.toHaveBeenCalled();
    expect(actions.pendingDelete).toBeNull();
  });
});
