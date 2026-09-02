import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

describe('ActionRunner', () => {
  let getRunningBranchActions: ReturnType<typeof vi.fn>;
  let getRunPhase: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    // The runner is a .svelte.ts module compiled without the Svelte plugin
    // here, so the rune calls resolve to these pass-through globals.
    vi.stubGlobal('$state', (initial: unknown) => initial);
    vi.stubGlobal('$derived', (value: unknown) => value);

    getRunningBranchActions = vi.fn();
    getRunPhase = vi.fn();
    vi.doMock('./actions', () => ({
      getRunningBranchActions,
      getRunPhase,
      stopBranchAction: vi.fn(),
      clearActionExecution: vi.fn(),
    }));
    vi.doMock('../../services/branchEventService', () => ({
      onBranchActionStatus: vi.fn(() => () => {}),
      onBranchRunPhaseChanged: vi.fn(() => () => {}),
    }));
    vi.doMock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.doUnmock('./actions');
    vi.doUnmock('../../services/branchEventService');
    vi.doUnmock('svelte-sonner');
  });

  function snapshot(executionId: string, actionType: string, phase: unknown) {
    return {
      executionId,
      branchId: 'repo:block/goose',
      actionId: `action-${executionId}`,
      actionName: executionId,
      actionType,
      startedAt: 7,
      phase,
    };
  }

  async function newRunner(opts: Record<string, unknown>) {
    const { ActionRunner } = await import('./actionRunner.svelte');
    return new ActionRunner({
      getScopeId: () => 'repo:block/goose',
      loadActions: async () => [],
      run: async () => 'execution-1',
      ...opts,
    } as never);
  }

  it('hydrates from an injected bulk loader without any per-execution phase calls', async () => {
    const buildingPhase = { type: 'building' };
    const loadRunning = vi
      .fn()
      .mockResolvedValue([
        snapshot('run-with-phase', 'run', buildingPhase),
        snapshot('run-without-phase', 'run', null),
        snapshot('test-without-phase', 'test', null),
      ]);

    const runner = await newRunner({ loadRunning });
    await runner.loadRunningActions();

    expect(loadRunning).toHaveBeenCalledTimes(1);
    expect(getRunningBranchActions).not.toHaveBeenCalled();
    expect(getRunPhase).not.toHaveBeenCalled();

    expect(runner.runningActions.map((a) => a.executionId)).toEqual([
      'run-with-phase',
      'run-without-phase',
      'test-without-phase',
    ]);
    expect(runner.runningActions.every((a) => a.status === 'running')).toBe(true);
    expect(runner.runPhases.get('run-with-phase')).toEqual(buildingPhase);
    // A phaseless run action still shows as running; other types get no phase.
    expect(runner.runPhases.get('run-without-phase')).toEqual({
      type: 'running',
      endpoint: null,
    });
    expect(runner.runPhases.has('test-without-phase')).toBe(false);

    // Re-hydrating leaves the tracked set alone rather than duplicating it.
    await runner.loadRunningActions();
    expect(runner.runningActions).toHaveLength(3);
  });

  it('falls back to per-scope and per-execution calls when no bulk loader is given', async () => {
    getRunningBranchActions.mockResolvedValue([snapshot('branch-run', 'run', undefined)]);
    getRunPhase.mockResolvedValue({ type: 'running', endpoint: 'http://localhost:5173' });

    const runner = await newRunner({});
    await runner.loadRunningActions();

    expect(getRunningBranchActions).toHaveBeenCalledWith('repo:block/goose');
    expect(getRunPhase).toHaveBeenCalledWith('branch-run');
    expect(runner.runPhases.get('branch-run')).toEqual({
      type: 'running',
      endpoint: 'http://localhost:5173',
    });
  });

  it('distinguishes an empty action list from a failed load', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      const empty = await newRunner({ loadActions: async () => [] });
      expect(await empty.loadActions()).toBe(true);

      const failing = await newRunner({
        loadActions: async () => {
          throw new Error('IPC unavailable');
        },
      });
      // A failure still empties the list, so the boolean is the only way a
      // caller can tell it apart from a context with no actions.
      expect(await failing.loadActions()).toBe(false);
      expect(failing.actions).toEqual([]);
    } finally {
      consoleError.mockRestore();
    }
  });

  it('hands each pinned button its own execution, and none to an idle one', async () => {
    const runner = await newRunner({});
    runner.runningActions.push(
      {
        executionId: 'execution-dev',
        actionId: 'action-dev',
        actionName: 'Dev',
        actionType: 'run',
        status: 'running',
      },
      {
        executionId: 'execution-test',
        actionId: 'action-test',
        actionName: 'Test',
        actionType: 'test',
        status: 'completed',
      }
    );

    // Several pinned actions can be in flight at once, so a button looks up
    // its own action rather than sharing one "the primary execution" slot.
    expect(runner.executionFor('action-dev')?.executionId).toBe('execution-dev');
    expect(runner.executionFor('action-test')?.status).toBe('completed');
    expect(runner.executionFor('action-build')).toBeNull();
  });

  it('focuses a running execution instead of starting a second one', async () => {
    const run = vi.fn().mockResolvedValue('execution-2');
    const runner = await newRunner({ run });
    runner.runningActions.push({
      executionId: 'execution-1',
      actionId: 'action-1',
      actionName: 'Dev',
      actionType: 'run',
      status: 'running',
    });

    await runner.runAction({ id: 'action-1', name: 'Dev' } as never);

    expect(run).not.toHaveBeenCalled();
    expect(runner.outputModal?.executionId).toBe('execution-1');
  });

  it('opens the modal only when a re-run starts the execution', async () => {
    const action = { id: 'action-1', name: 'Dev' };
    const run = vi.fn().mockResolvedValue('execution-2');

    // The run button starts the action without pulling up the modal...
    const fromButton = await newRunner({ run });
    await fromButton.runAction(action as never);
    expect(run).toHaveBeenCalledWith('action-1');
    expect(fromButton.outputModal).toBeNull();

    // ...while Run Again repoints the open modal at the new execution.
    const fromModal = await newRunner({ loadActions: async () => [action], run });
    await fromModal.loadActions();
    fromModal.outputModal = {
      executionId: 'execution-1',
      actionId: 'action-1',
      actionName: 'Dev',
      isStopping: false,
    };
    await fromModal.runAgain();
    expect(fromModal.outputModal).toEqual({
      executionId: 'execution-2',
      actionId: 'action-1',
      actionName: 'Dev',
      isStopping: false,
    });
  });
});
