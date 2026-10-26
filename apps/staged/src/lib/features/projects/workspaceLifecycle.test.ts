import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

describe('WorkspaceLifecycleController.retryWorktree', () => {
  let setupWorktreeAndRunPrerun: ReturnType<typeof vi.fn>;
  let commandsMock: Record<string, ReturnType<typeof vi.fn>>;
  let branchesByProject: Map<
    string,
    Array<{ id: string; worktreePath?: string; branchType?: string; workspaceStatus?: string }>
  >;

  beforeEach(() => {
    vi.resetModules();
    // The controller is a .svelte.ts module compiled without the Svelte plugin
    // here, so the rune calls resolve to these pass-through globals.
    vi.stubGlobal('$state', (initial: unknown) => initial);
    vi.stubGlobal('$derived', (value: unknown) => value);

    setupWorktreeAndRunPrerun = vi.fn();
    // Only the commands the controller legitimately issues. Anything else it
    // reaches for (a queue drain, say) fails loudly as a missing export.
    commandsMock = {
      setupWorktree: vi.fn(),
      setupWorktreeAndRunPrerun,
      pollAllWorkspaceStatuses: vi.fn(async () => ({})),
    };
    vi.doMock('../../api/commands', () => commandsMock);
    vi.doMock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));
    vi.doMock('../settings/preferences.svelte', () => ({
      getPreferredAgent: () => 'claude',
    }));
    vi.doMock('../agents/agent.svelte', () => ({ agentState: { providers: [] } }));

    branchesByProject = new Map([['project-1', [{ id: 'branch-1' }]]]);
  });

  afterEach(() => {
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
    });
    return workspaceLifecycle;
  }

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
