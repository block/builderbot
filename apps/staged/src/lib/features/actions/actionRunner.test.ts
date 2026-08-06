import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

describe('ActionRunner running-state hydration', () => {
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
});
