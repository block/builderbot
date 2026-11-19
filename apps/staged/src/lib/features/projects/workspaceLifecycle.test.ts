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
  let eventListeners: Map<string, Array<(payload: unknown) => void>>;
  let toastError: ReturnType<typeof vi.fn>;
  let toastWarning: ReturnType<typeof vi.fn>;

  /** Deliver a backend event to the controller's listeners. */
  function emit(event: string, payload: unknown) {
    for (const listener of eventListeners.get(event) ?? []) listener(payload);
  }

  const STARTING = { status: 'starting', workstationId: null };

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
      startWorkspace: vi.fn(async () => undefined),
      pollAllWorkspaceStatuses: vi.fn(async () => ({})),
      getWorkspaceSetupErrors: vi.fn(async () => ({})),
    };
    vi.doMock('../../api/commands', () => commandsMock);
    eventListeners = new Map();
    vi.doMock('../../transport', () => ({
      listenToEvent: (
        event: string,
        callback: (payload: unknown) => void,
        opts?: { onEstablished?: () => void }
      ) => {
        const listeners = eventListeners.get(event) ?? [];
        listeners.push(callback);
        eventListeners.set(event, listeners);
        void Promise.resolve().then(() => opts?.onEstablished?.());
        return () => {
          eventListeners.set(
            event,
            (eventListeners.get(event) ?? []).filter((l) => l !== callback)
          );
        };
      },
    }));
    vi.doMock('svelte-sonner', () => {
      toastError = vi.fn();
      toastWarning = vi.fn();
      return { toast: { error: toastError, warning: toastWarning } };
    });
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
    vi.doUnmock('../../transport');
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

  // The call only asks. A rejection means the request could not be made and
  // says nothing about the branch: its status is the poll's to report and
  // its reason the backend's, so the reasons are read again.
  it('does not read a rejected startWorkspace as a failed setup', async () => {
    vi.useFakeTimers();
    commandsMock.startWorkspace.mockRejectedValue(new Error('Branch has no workspace name'));
    branchesByProject = new Map([
      ['project-1', [{ id: 'branch-1', branchType: 'remote', workspaceStatus: 'starting' }]],
    ]);
    const lifecycle = await newController();
    await settle();
    commandsMock.getWorkspaceSetupErrors.mockResolvedValue({ 'branch-1': 'could not fetch' });
    await kickoff(lifecycle);

    expect(branchesByProject.get('project-1')?.[0].workspaceStatus).toBe('starting');
    expect(commandsMock.getWorkspaceSetupErrors).toHaveBeenCalledTimes(2);
    expect(lifecycle.getWorkspaceErrors().get('branch-1')).toBe('could not fetch');
    expect(toastError).not.toHaveBeenCalled();
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
    // The setup-error snapshot taken at start is a read, not a drain.
    const issued = Object.entries(commandsMock)
      .filter(([name, fn]) => fn.mock.calls.length > 0 && name !== 'getWorkspaceSetupErrors')
      .map(([name]) => name);
    expect(issued).toEqual(['pollAllWorkspaceStatuses']);
    lifecycle.stop();
  });
  /** Remote branches in `starting`, as the store would hold them. */
  function startingBranches(...ids: string[]) {
    return ids.map((id) => ({ id, branchType: 'remote', workspaceStatus: 'starting' }));
  }

  function statusOf(branchId: string) {
    return branchesByProject.get('project-1')?.find((b) => b.id === branchId)?.workspaceStatus;
  }

  // The backend runs the setup and bounds it. Every `starting` branch is
  // asked for at once and once, nothing here times a slow one out, and the
  // poll reports how each ends.
  it('asks for every starting branch once and leaves the outcome to the poll', async () => {
    vi.useFakeTimers();
    const ids = ['branch-1', 'branch-2', 'branch-3', 'branch-4', 'branch-5'];
    // One request never returns: it holds up no other and is not re-issued.
    commandsMock.startWorkspace.mockImplementation((branchId: string) =>
      branchId === 'branch-5' ? new Promise(() => {}) : Promise.resolve()
    );
    let polled: Record<string, { status: string; workstationId: number | null }> =
      Object.fromEntries(ids.map((id) => [id, STARTING]));
    commandsMock.pollAllWorkspaceStatuses.mockImplementation(async () => polled);
    branchesByProject = new Map([['project-1', startingBranches(...ids)]]);
    const lifecycle = await newController();
    await kickoff(lifecycle);

    expect(commandsMock.startWorkspace.mock.calls.map(([id]) => id)).toEqual(ids);
    await kickoff(lifecycle);
    expect(commandsMock.startWorkspace.mock.calls.map(([id]) => id)).toEqual([
      ...ids,
      ...ids.slice(0, 4),
    ]);

    await vi.advanceTimersByTimeAsync(7 * 60 * 1000);
    expect(ids.map(statusOf)).toEqual(ids.map(() => 'starting'));
    expect(lifecycle.getWorkspaceErrors().size).toBe(0);

    polled = {
      ...polled,
      'branch-1': { status: 'running', workstationId: 3 },
      'branch-2': { status: 'error', workstationId: null },
    };
    await vi.advanceTimersByTimeAsync(3000);
    expect(branchesByProject.get('project-1')?.slice(0, 2)).toEqual([
      { id: 'branch-1', branchType: 'remote', workspaceStatus: 'running', workstationId: 3 },
      { id: 'branch-2', branchType: 'remote', workspaceStatus: 'error' },
    ]);
    expect(toastError).not.toHaveBeenCalled();
    lifecycle.stop();
  });

  // A list refetched with a branch back in `starting` (a Retry or a resume
  // from another window) asks for it and polls it, even with the poller
  // stopped.
  it('asks for and polls a branch a refetched list shows starting again', async () => {
    vi.useFakeTimers();
    branchesByProject = new Map([
      ['project-1', [{ id: 'branch-1', branchType: 'remote', workspaceStatus: 'error' }]],
    ]);
    const lifecycle = await newController();
    await kickoff(lifecycle);
    expect(commandsMock.startWorkspace).not.toHaveBeenCalled();
    expect(commandsMock.pollAllWorkspaceStatuses).not.toHaveBeenCalled();

    branchesByProject = new Map([['project-1', startingBranches('branch-1')]]);
    await kickoff(lifecycle);
    expect(commandsMock.startWorkspace).toHaveBeenCalledWith('branch-1');
    expect(commandsMock.pollAllWorkspaceStatuses).toHaveBeenCalledWith(['branch-1']);
    lifecycle.stop();
  });

  it('retryWorkspace asks again for a branch in error, once at a time', async () => {
    vi.useFakeTimers();
    branchesByProject = new Map([
      ['project-1', [{ id: 'branch-1', branchType: 'remote', workspaceStatus: 'error' }]],
    ]);
    const lifecycle = await newController();
    await settle();
    emit('workspace-setup-error', { branchId: 'branch-1', error: 'could not fetch' });
    expect(lifecycle.getWorkspaceErrors().get('branch-1')).toBe('could not fetch');

    let finishRequest!: () => void;
    commandsMock.startWorkspace.mockReturnValueOnce(
      new Promise<void>((resolve) => (finishRequest = resolve))
    );
    lifecycle.retryWorkspace('project-1', 'branch-1');
    expect(statusOf('branch-1')).toBe('starting');
    expect(lifecycle.getWorkspaceErrors().has('branch-1')).toBe(false);
    expect(commandsMock.pollAllWorkspaceStatuses).toHaveBeenCalledWith(['branch-1']);

    lifecycle.retryWorkspace('project-1', 'branch-1');
    expect(commandsMock.startWorkspace).toHaveBeenCalledTimes(1);

    finishRequest();
    await settle();
    lifecycle.retryWorkspace('project-1', 'branch-1');
    expect(commandsMock.startWorkspace).toHaveBeenCalledTimes(2);
    lifecycle.stop();
  });

  // The backend says whose run failed. Its own (an automatic retry, the
  // startup pass, a poll-triggered run) only updates the card; one the user
  // asked for (a new branch, Retry, Resume) also raises a toast.
  it('shows an automatic retry failing on the card without a toast', async () => {
    const lifecycle = await newController();
    await settle();

    emit('workspace-setup-error', {
      branchId: 'branch-1',
      error: 'could not fetch',
      userRequested: false,
    });
    emit('workspace-setup-error', { branchId: 'branch-2', error: 'workstation never booted' });
    expect([...lifecycle.getWorkspaceErrors()]).toEqual([
      ['branch-1', 'could not fetch'],
      ['branch-2', 'workstation never booted'],
    ]);
    expect(toastError).not.toHaveBeenCalled();

    emit('workspace-setup-error', {
      branchId: 'branch-1',
      error: 'still could not fetch',
      userRequested: true,
    });
    expect(lifecycle.getWorkspaceErrors().get('branch-1')).toBe('still could not fetch');
    expect(toastError.mock.calls).toEqual([
      ['Unable to start workspace', { description: 'still could not fetch', duration: Infinity }],
    ]);
    lifecycle.stop();
  });

  // A setup that put commits it found on the base branch on this branch
  // worked, so there is no reason for the card. The user is still told,
  // since the log is all that said so before.
  it('tells the user when a setup brought commits along to the branch', async () => {
    const lifecycle = await newController();
    await settle();
    emit('workspace-setup-error', { branchId: 'branch-1', error: 'could not fetch' });

    const notice = "Branch 'feature' was created from the workstation's local 'main'";
    emit('workspace-setup-error', {
      branchId: 'branch-1',
      error: null,
      userRequested: false,
      notice,
    });
    expect(lifecycle.getWorkspaceErrors().has('branch-1')).toBe(false);
    expect(toastWarning.mock.calls).toEqual([
      ['Workspace branch was given existing commits', { description: notice, duration: Infinity }],
    ]);
    expect(toastError).not.toHaveBeenCalled();

    // A reason cleared with nothing to tell raises nothing.
    emit('workspace-setup-error', { branchId: 'branch-1', error: null });
    expect(toastWarning).toHaveBeenCalledTimes(1);
    lifecycle.stop();
  });

  // A queued session held behind a sibling branch's session stays queued
  // with no error. The backend says why on the same event, under a heading
  // of its own, and the branch is given no reason to show.
  it('tells the user why a queued session is waiting, under the heading it came with', async () => {
    const lifecycle = await newController();
    await settle();

    const notice =
      "A queued session of branch 'feature' is waiting: a session of branch 'sibling' is " +
      'running in the same clone. It starts once that session has finished.';
    emit('workspace-setup-error', {
      branchId: 'branch-1',
      error: null,
      userRequested: false,
      notice,
      noticeTitle: 'Queued session is waiting',
    });
    expect(toastWarning.mock.calls).toEqual([
      ['Queued session is waiting', { description: notice, duration: Infinity }],
    ]);
    expect(toastError).not.toHaveBeenCalled();
    expect(lifecycle.getWorkspaceErrors().has('branch-1')).toBe(false);
    lifecycle.stop();
  });

  // The backend reports every failed run, and a snapshot covers the ones
  // from before the window mounted.
  it('keeps setup reasons from the error event and the snapshot taken at start', async () => {
    let finishSnapshot!: (errors: Record<string, string>) => void;
    commandsMock.getWorkspaceSetupErrors.mockReturnValue(
      new Promise((resolve) => (finishSnapshot = resolve))
    );
    const lifecycle = await newController();
    await settle();
    expect(commandsMock.getWorkspaceSetupErrors).toHaveBeenCalledTimes(1);

    // An event since the snapshot was asked for is newer than it.
    emit('workspace-setup-error', { branchId: 'branch-2', error: null });
    finishSnapshot({ 'branch-1': 'workstation never booted', 'branch-2': 'stale' });
    await settle();
    expect([...lifecycle.getWorkspaceErrors()]).toEqual([['branch-1', 'workstation never booted']]);

    emit('workspace-setup-error', { branchId: 'branch-3', error: 'could not fetch' });
    expect(lifecycle.getWorkspaceErrors().get('branch-3')).toBe('could not fetch');
    emit('workspace-setup-error', { branchId: 'branch-3', error: null });
    expect(lifecycle.getWorkspaceErrors().has('branch-3')).toBe(false);

    // Stopped: the listener is gone.
    lifecycle.stop();
    expect(eventListeners.get('workspace-setup-error')).toEqual([]);
  });
});
