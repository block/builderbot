import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

describe('ActionRunner', () => {
  let getRunningBranchActions: ReturnType<typeof vi.fn>;
  let getRunPhase: ReturnType<typeof vi.fn>;
  let clearActionExecution: ReturnType<typeof vi.fn>;
  /** The runner's action_status handler, once it has subscribed. */
  let emitStatus: (payload: Record<string, unknown>) => void;

  beforeEach(() => {
    vi.resetModules();
    // The runner is a .svelte.ts module compiled without the Svelte plugin
    // here, so the rune calls resolve to these pass-through globals.
    vi.stubGlobal('$state', (initial: unknown) => initial);
    vi.stubGlobal('$derived', (value: unknown) => value);

    getRunningBranchActions = vi.fn();
    getRunPhase = vi.fn();
    clearActionExecution = vi.fn().mockResolvedValue(true);
    emitStatus = () => {
      throw new Error('the runner has not subscribed');
    };
    vi.doMock('./actions', () => ({
      ACTION_TYPES: ['run', 'prerun', 'build', 'test', 'format', 'check', 'cleanUp'],
      getRunningBranchActions,
      getRunPhase,
      stopBranchAction: vi.fn(),
      clearActionExecution,
    }));
    vi.doMock('../../services/branchEventService', () => ({
      onBranchActionStatus: vi.fn((_scopeId: string, handler: typeof emitStatus) => {
        emitStatus = handler;
        return () => {};
      }),
      onBranchRunPhaseChanged: vi.fn(() => () => {}),
    }));
    vi.doMock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));
  });

  afterEach(() => {
    vi.useRealTimers();
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

  describe('buffered output of finished executions', () => {
    /** How long a finished pill stays, fade included. */
    const COMPLETED_PILL_MS = 2000 + 300;
    const FAILED_PILL_MS = 3000 + 300;

    function status(executionId: string, value: string, actionId = `action-${executionId}`) {
      return {
        executionId,
        branchId: 'repo:block/goose',
        actionId,
        actionName: executionId,
        actionType: 'test',
        status: value,
        exitCode: value === 'completed' ? 0 : value === 'failed' ? 1 : undefined,
        startedAt: 1,
        completedAt: value === 'running' ? undefined : 2,
      };
    }

    /** A subscribed runner that has seen `executionId` start. */
    async function runnerWithRunning(executionId: string, opts: Record<string, unknown> = {}) {
      vi.useFakeTimers();
      const runner = await newRunner(opts);
      runner.subscribe();
      emitStatus(status(executionId, 'running'));
      return runner;
    }

    it('frees the output when the pill is dropped, and not before', async () => {
      const runner = await runnerWithRunning('execution-1');

      emitStatus(status('execution-1', 'completed'));
      vi.advanceTimersByTime(COMPLETED_PILL_MS - 1);
      // The pill is still there to be clicked, so the output must be too.
      expect(runner.runningActions).toHaveLength(1);
      expect(clearActionExecution).not.toHaveBeenCalled();

      vi.advanceTimersByTime(1);
      expect(runner.runningActions).toHaveLength(0);
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
      expect(clearActionExecution).toHaveBeenCalledWith('execution-1');
    });

    it('frees the output of failed and stopped executions on their longer schedule', async () => {
      const runner = await runnerWithRunning('execution-failed');
      emitStatus(status('execution-stopped', 'running'));

      emitStatus(status('execution-failed', 'failed'));
      emitStatus(status('execution-stopped', 'stopped'));
      vi.advanceTimersByTime(FAILED_PILL_MS - 1);
      expect(clearActionExecution).not.toHaveBeenCalled();

      vi.advanceTimersByTime(1);
      expect(runner.runningActions).toHaveLength(0);
      expect(clearActionExecution.mock.calls.map(([id]) => id).sort()).toEqual([
        'execution-failed',
        'execution-stopped',
      ]);
    });

    it('leaves the output of a running execution alone', async () => {
      const runner = await runnerWithRunning('execution-1');
      runner.showOutput(runner.runningActions[0]);
      runner.closeOutputModal();
      vi.advanceTimersByTime(60_000);

      expect(runner.runningActions).toHaveLength(1);
      expect(clearActionExecution).not.toHaveBeenCalled();
    });

    it('ignores the end of an execution it never listed', async () => {
      await runnerWithRunning('execution-1');
      emitStatus(status('execution-unknown', 'completed'));
      vi.advanceTimersByTime(60_000);

      expect(clearActionExecution).not.toHaveBeenCalled();
    });

    it('keeps the output while the modal is showing it, and frees it on close', async () => {
      const runner = await runnerWithRunning('execution-1');
      runner.showOutput(runner.runningActions[0]);

      emitStatus(status('execution-1', 'completed'));
      vi.advanceTimersByTime(COMPLETED_PILL_MS + 60_000);
      // The pill has gone, but the modal is still on this execution.
      expect(runner.runningActions).toHaveLength(0);
      expect(clearActionExecution).not.toHaveBeenCalled();

      runner.closeOutputModal();
      expect(runner.outputModal).toBeNull();
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
      expect(clearActionExecution).toHaveBeenCalledWith('execution-1');
    });

    it("keeps the output when the modal opens in the pill's last moments", async () => {
      const runner = await runnerWithRunning('execution-1');

      emitStatus(status('execution-1', 'completed'));
      vi.advanceTimersByTime(COMPLETED_PILL_MS - 1);
      runner.showOutput(runner.runningActions[0]);
      vi.advanceTimersByTime(1);

      expect(runner.runningActions).toHaveLength(0);
      expect(clearActionExecution).not.toHaveBeenCalled();
      runner.closeOutputModal();
      expect(clearActionExecution).toHaveBeenCalledWith('execution-1');
    });

    it('frees the output when the modal moves on to another execution', async () => {
      const runner = await runnerWithRunning('execution-1');
      emitStatus(status('execution-2', 'running'));
      runner.showOutput(runner.runningActions[0]);

      emitStatus(status('execution-1', 'completed'));
      vi.advanceTimersByTime(COMPLETED_PILL_MS);
      expect(clearActionExecution).not.toHaveBeenCalled();

      runner.showOutput(runner.runningActions[0]);
      expect(runner.outputModal?.executionId).toBe('execution-2');
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
      expect(clearActionExecution).toHaveBeenCalledWith('execution-1');

      // Closing the modal on the other, running, execution frees nothing more.
      runner.closeOutputModal();
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
    });

    it('does not free the output again when the modal is closed a second time', async () => {
      const runner = await runnerWithRunning('execution-1');
      runner.showOutput(runner.runningActions[0]);
      emitStatus(status('execution-1', 'completed'));
      vi.advanceTimersByTime(COMPLETED_PILL_MS);

      runner.closeOutputModal();
      runner.closeOutputModal();
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
    });

    it('frees a finished run once when its action is run again before the pill goes', async () => {
      const action = { id: 'action-1', name: 'Test' };
      const run = vi.fn().mockResolvedValue('execution-2');
      const runner = await runnerWithRunning('execution-1', { run });
      emitStatus(status('execution-1', 'completed', 'action-1'));
      // The first run started under its own action id; give it this one.
      runner.runningActions[0].actionId = 'action-1';

      await runner.runAction(action as never);
      expect(runner.runningActions).toHaveLength(0);
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
      expect(clearActionExecution).toHaveBeenCalledWith('execution-1');

      // The first run's pill timers still fire, with nothing left to do.
      vi.advanceTimersByTime(FAILED_PILL_MS);
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
    });

    it('keeps a finished run the modal is showing until Run Again has replaced it', async () => {
      const action = { id: 'action-1', name: 'Test' };
      let started: (executionId: string) => void = () => {};
      const run = vi.fn(() => new Promise<string>((resolve) => void (started = resolve)));
      const runner = await runnerWithRunning('execution-1', {
        loadActions: async () => [action],
        run,
      });
      await runner.loadActions();
      runner.runningActions[0].actionId = 'action-1';
      runner.showOutput(runner.runningActions[0]);
      emitStatus(status('execution-1', 'failed', 'action-1'));

      // Run Again drops the finished run from the list while the modal is
      // still showing its output, which is kept until the new run is up.
      const rerun = runner.runAgain();
      expect(runner.runningActions).toHaveLength(0);
      expect(clearActionExecution).not.toHaveBeenCalled();

      started('execution-2');
      await rerun;
      expect(runner.outputModal?.executionId).toBe('execution-2');
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
      expect(clearActionExecution).toHaveBeenCalledWith('execution-1');
    });

    it('survives a clear that fails', async () => {
      clearActionExecution.mockRejectedValue(new Error('IPC unavailable'));
      const runner = await runnerWithRunning('execution-1');

      emitStatus(status('execution-1', 'completed'));
      vi.advanceTimersByTime(COMPLETED_PILL_MS);
      await vi.runAllTimersAsync();

      expect(runner.runningActions).toHaveLength(0);
      expect(clearActionExecution).toHaveBeenCalledTimes(1);
    });
  });
});
