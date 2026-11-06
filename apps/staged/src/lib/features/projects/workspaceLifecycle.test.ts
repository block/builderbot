import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';

describe('WorkspaceLifecycleController.retryWorktree', () => {
  let setupWorktreeAndRunPrerun: ReturnType<typeof vi.fn>;
  let commandsMock: Record<string, ReturnType<typeof vi.fn>>;
  let branchesByProject: Map<
    string,
    Array<{ id: string; worktreePath?: string; branchType?: string; workspaceStatus?: string }>
  >;
  let transferringProjects: Set<string>;
  let projectLocations: Map<string, string>;
  let refreshTransferredProject: Mock<(projectId: string) => Promise<void>>;

  beforeEach(() => {
    vi.resetModules();
    // The controller is a .svelte.ts module compiled without the Svelte plugin
    // here, so the rune calls resolve to these pass-through globals.
    vi.stubGlobal('$state', (initial: unknown) => initial);
    vi.stubGlobal('$derived', (value: unknown) => value);

    setupWorktreeAndRunPrerun = vi.fn();
    // Every command the controller may issue, each a spy, so a test can
    // assert one wasn't: a missing export would throw inside a setup task's
    // try and be swallowed as a setup error, never failing the test.
    commandsMock = {
      setupWorktree: vi.fn(),
      setupWorktreeAndRunPrerun,
      startWorkspace: vi.fn(async () => {}),
      pollAllWorkspaceStatuses: vi.fn(async () => ({})),
    };
    vi.doMock('../../api/commands', () => commandsMock);
    vi.doMock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));
    vi.doMock('../settings/preferences.svelte', () => ({
      getPreferredAgent: () => 'claude',
    }));
    vi.doMock('../agents/agent.svelte', () => ({ agentState: { providers: [] } }));

    branchesByProject = new Map([['project-1', [{ id: 'branch-1' }]]]);
    transferringProjects = new Set();
    projectLocations = new Map();
    refreshTransferredProject = vi.fn(async (_projectId: string) => {});
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.doUnmock('../../api/commands');
    vi.doUnmock('svelte-sonner');
    vi.doUnmock('../settings/preferences.svelte');
    vi.doUnmock('../agents/agent.svelte');
  });

  async function newController() {
    const { workspaceLifecycle } = await import('./workspaceLifecycle.svelte');
    workspaceLifecycle.start({
      getBranchesByProject: () => branchesByProject as never,
      setBranchesByProject: (next) => {
        branchesByProject = next as never;
      },
      isProjectDeleting: () => false,
      isProjectTransferring: (projectId) => transferringProjects.has(projectId),
      getProjectLocation: (projectId) => (projectLocations.get(projectId) as never) ?? null,
      refreshTransferredProject: (projectId) => refreshTransferredProject(projectId),
    });
    return workspaceLifecycle;
  }

  /** Flush the setup queue's microtasks. */
  async function settle() {
    for (let i = 0; i < 4; i++) await Promise.resolve();
  }

  /** Run a scheduled kickoff (50 ms debounce) and the setup it queues. */
  async function kickoff(lifecycle: { scheduleKickoff: () => void }) {
    lifecycle.scheduleKickoff();
    await vi.advanceTimersByTimeAsync(60);
    await settle();
  }

  /** What each project-level setup command was called with, by name. */
  function setupCalls() {
    return {
      setupWorktree: commandsMock.setupWorktree.mock.calls,
      setupWorktreeAndRunPrerun: commandsMock.setupWorktreeAndRunPrerun.mock.calls,
      startWorkspace: commandsMock.startWorkspace.mock.calls,
    };
  }

  // The backend refuses setup_worktree / start_workspace for a transferring
  // project, so the frontend must not ask: a transfer's branches land
  // unprovisioned (local without a worktree, remote still starting) and are
  // the transfer's until it ends. Then the lists the store holds still
  // describe the old location (the commit's refetch is frame-buffered), so
  // setup waits for the project's refetch and provisions from that.
  it('holds a transferred project until its refetch, then sets it up from the fresh lists', async () => {
    vi.useFakeTimers();
    // As the transfers end: project-1 went to local and project-2 to Blox,
    // but neither the project list nor the branch lists have caught up.
    // project-2's branch had no worktree (its setup failed before), so set
    // up from this stale list it would get a local worktree again.
    branchesByProject = new Map([
      ['project-1', [{ id: 'branch-1', branchType: 'remote', workspaceStatus: 'running' }]],
      ['project-2', [{ id: 'branch-2', branchType: 'local' }]],
    ]);
    projectLocations = new Map([
      ['project-1', 'remote'],
      ['project-2', 'local'],
    ]);
    transferringProjects = new Set(['project-1', 'project-2']);
    const lifecycle = await newController();

    lifecycle.enqueueInitialSetup('project-1', branchesByProject.get('project-1') as never);
    lifecycle.enqueueInitialSetup('project-2', branchesByProject.get('project-2') as never);
    await kickoff(lifecycle);
    // Nor are its workspaces polled: the transfer creates and deletes them.
    expect(commandsMock.pollAllWorkspaceStatuses).not.toHaveBeenCalled();

    // Still transferring: transferFinished is a no-op (a stale call).
    lifecycle.transferFinished('project-1');
    expect(refreshTransferredProject).not.toHaveBeenCalled();

    let finishRefresh!: () => void;
    refreshTransferredProject.mockReturnValue(
      new Promise<void>((resolve) => (finishRefresh = resolve))
    );
    transferringProjects.clear();
    lifecycle.transferFinished('project-1');
    lifecycle.transferFinished('project-2');
    expect(refreshTransferredProject.mock.calls).toEqual([['project-1'], ['project-2']]);

    // Held while the refetch is in flight, whatever triggers setup.
    lifecycle.enqueueInitialSetup('project-2', branchesByProject.get('project-2') as never);
    await kickoff(lifecycle);
    expect(setupCalls()).toEqual({
      setupWorktree: [],
      setupWorktreeAndRunPrerun: [],
      startWorkspace: [],
    });

    // The refetch lands: project-1's branch had nothing to carry and has no
    // worktree yet, project-2's workspace is still to start.
    commandsMock.setupWorktreeAndRunPrerun.mockResolvedValue({
      id: 'branch-1',
      branchType: 'local',
      worktreePath: '/tmp/wt',
    });
    branchesByProject = new Map([
      ['project-1', [{ id: 'branch-1', branchType: 'local' }]],
      ['project-2', [{ id: 'branch-2', branchType: 'remote', workspaceStatus: 'starting' }]],
    ]);
    projectLocations = new Map([
      ['project-1', 'local'],
      ['project-2', 'remote'],
    ]);
    finishRefresh();
    await vi.advanceTimersByTimeAsync(60);
    await settle();

    // Set up like a newly created branch: with its prerun actions.
    expect(setupCalls()).toEqual({
      setupWorktree: [],
      setupWorktreeAndRunPrerun: [['branch-1', 'claude']],
      startWorkspace: [['branch-2']],
    });
    expect(branchesByProject.get('project-1')).toEqual([
      { id: 'branch-1', branchType: 'local', worktreePath: '/tmp/wt' },
    ]);
    expect(lifecycle.getWorkspaceErrors().size).toBe(0);
    lifecycle.stop();
  });

  it('skips a branch whose type does not match its project location', async () => {
    vi.useFakeTimers();
    commandsMock.setupWorktree.mockResolvedValue({
      id: 'branch-1',
      branchType: 'local',
      worktreePath: '/tmp/wt',
    });
    branchesByProject = new Map([['project-1', [{ id: 'branch-1', branchType: 'local' }]]]);
    projectLocations = new Map([['project-1', 'remote']]);
    const lifecycle = await newController();

    await kickoff(lifecycle);
    expect(commandsMock.setupWorktree).not.toHaveBeenCalled();

    // Once the two agree it is an ordinary branch: no prerun from here.
    projectLocations.set('project-1', 'local');
    await kickoff(lifecycle);
    expect(commandsMock.setupWorktree).toHaveBeenCalledWith('branch-1');
    expect(commandsMock.setupWorktreeAndRunPrerun).not.toHaveBeenCalled();
    lifecycle.stop();
  });

  it('transferFinished clears errors the old location left on the branches', async () => {
    vi.useFakeTimers();
    commandsMock.setupWorktreeAndRunPrerun.mockRejectedValueOnce(new Error('no clone'));
    branchesByProject = new Map([['project-1', [{ id: 'branch-1', branchType: 'local' }]]]);
    const lifecycle = await newController();
    await lifecycle.retryWorktree('branch-1', 'project-1').catch(() => {});
    expect(lifecycle.getWorktreeErrors().get('branch-1')).toBe('no clone');

    commandsMock.setupWorktreeAndRunPrerun.mockResolvedValue({
      id: 'branch-1',
      worktreePath: '/tmp/wt',
    });
    lifecycle.transferFinished('project-1');
    expect(lifecycle.getWorktreeErrors().has('branch-1')).toBe(false);
    await vi.advanceTimersByTimeAsync(60);
    await settle();
    expect(commandsMock.setupWorktreeAndRunPrerun).toHaveBeenLastCalledWith('branch-1', 'claude');
    lifecycle.stop();
  });

  // While the command is in flight the branch really is held, so the release
  // asserted below is a release and not a guard that never engaged.
  it('holds the branch while the worktree command is in flight', async () => {
    let resolveSetup: (value: unknown) => void = () => {};
    setupWorktreeAndRunPrerun.mockReturnValue(
      new Promise((resolve) => {
        resolveSetup = resolve;
      })
    );

    const lifecycle = await newController();
    const inFlight = lifecycle.retryWorktree('branch-1', 'project-1');
    await lifecycle.retryWorktree('branch-1', 'project-1');

    expect(setupWorktreeAndRunPrerun).toHaveBeenCalledTimes(1);

    resolveSetup({ id: 'branch-1', worktreePath: '/tmp/wt' });
    await inFlight;
    lifecycle.stop();
  });

  // The command now resolves at worktree-ready — prerun is detached in the
  // backend — so the retry promise means "the worktree exists", and the
  // branch must be free for another retry the moment it settles rather than
  // being pinned for the length of a setup-action run.
  it('applies the worktree path and frees the branch when the command resolves', async () => {
    setupWorktreeAndRunPrerun.mockResolvedValue({ id: 'branch-1', worktreePath: '/tmp/wt' });

    const lifecycle = await newController();
    await lifecycle.retryWorktree('branch-1', 'project-1');

    expect(branchesByProject.get('project-1')).toEqual([
      { id: 'branch-1', worktreePath: '/tmp/wt' },
    ]);

    await lifecycle.retryWorktree('branch-1', 'project-1');
    expect(setupWorktreeAndRunPrerun).toHaveBeenCalledTimes(2);
    lifecycle.stop();
  });

  // The backend kicks the branch queue when it writes the `running` workspace
  // status, so the frontend must not issue its own drain on the transition —
  // the only command a `running` transition may trigger is the status poll.
  it('issues no queue drain when a workspace becomes running', async () => {
    branchesByProject = new Map([
      ['project-1', [{ id: 'branch-1', branchType: 'remote', workspaceStatus: 'starting' }]],
    ]);

    const lifecycle = await newController();
    lifecycle.handleWorkspaceStatusChange('project-1', 'branch-1', 'running');
    await Promise.resolve();

    expect(branchesByProject.get('project-1')).toEqual([
      { id: 'branch-1', branchType: 'remote', workspaceStatus: 'running' },
    ]);
    const issued = Object.entries(commandsMock)
      .filter(([, fn]) => fn.mock.calls.length > 0)
      .map(([name]) => name);
    expect(issued).toEqual(['pollAllWorkspaceStatuses']);
    lifecycle.stop();
  });
});
