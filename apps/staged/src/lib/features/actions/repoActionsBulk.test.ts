import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

describe('repo card bulk action hydration', () => {
  let invokeCommand: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    invokeCommand = vi.fn();
    vi.doMock('../../transport', () => ({ invokeCommand, isTauri: true }));
  });

  afterEach(() => {
    vi.doUnmock('../../transport');
  });

  function action(name: string) {
    return {
      id: `action-${name}`,
      contextId: `context-${name}`,
      name,
      command: name,
      actionType: 'run',
      sortOrder: 0,
      autoCommit: false,
      createdAt: 0,
      updatedAt: 0,
    };
  }

  function execution(executionId: string, scopeId: string, phase: unknown = null) {
    return {
      executionId,
      branchId: scopeId,
      actionId: `action-${executionId}`,
      actionName: executionId,
      actionType: 'run',
      startedAt: 1,
      phase,
    };
  }

  it('folds same-tick callers into one list_all_repo_actions call, sliced per repo', async () => {
    invokeCommand.mockResolvedValue([
      {
        githubRepo: 'block/builderbot',
        subpath: 'apps/staged',
        actions: [action('dev')],
      },
      { githubRepo: 'block/goose', subpath: null, actions: [action('build')] },
    ]);

    const { bulkRepoActions } = await import('./repoActionsBulk');

    const [staged, goose, gooseEmptySubpath, contextless] = await Promise.all([
      bulkRepoActions('block/builderbot', 'apps/staged'),
      bulkRepoActions('block/goose'),
      // An empty subpath keys the same as no subpath at all.
      bulkRepoActions('block/goose', ''),
      bulkRepoActions('block/never-configured'),
    ]);

    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(invokeCommand).toHaveBeenCalledWith('list_all_repo_actions');
    expect(staged.map((a) => a.name)).toEqual(['dev']);
    expect(goose.map((a) => a.name)).toEqual(['build']);
    expect(gooseEmptySubpath).toEqual(goose);
    // No context row yet reads as "no actions", not as an error.
    expect(contextless).toEqual([]);
  });

  it('starts a fresh wave once the previous one has resolved', async () => {
    invokeCommand.mockResolvedValue([]);

    const { bulkRepoActions } = await import('./repoActionsBulk');

    await bulkRepoActions('block/goose');
    await bulkRepoActions('block/goose');

    expect(invokeCommand).toHaveBeenCalledTimes(2);
  });

  it('rejects the callers of a failed wave without wedging the next one', async () => {
    invokeCommand
      .mockRejectedValueOnce(new Error('store not initialized'))
      .mockResolvedValueOnce([
        { githubRepo: 'block/goose', subpath: null, actions: [action('build')] },
      ]);

    const { bulkRepoActions } = await import('./repoActionsBulk');

    await expect(bulkRepoActions('block/goose')).rejects.toThrow('store not initialized');

    const retried = await bulkRepoActions('block/goose');
    expect(retried.map((a) => a.name)).toEqual(['build']);
  });

  it('keys running executions by scope id, phase included', async () => {
    const phase = { type: 'running', endpoint: 'http://localhost:3000' };
    invokeCommand.mockResolvedValue([
      execution('repo-run', 'repo:block/goose', phase),
      execution('repo-test', 'repo:block/goose'),
      execution('branch-run', 'branch-1'),
    ]);

    const { bulkRunningForScope } = await import('./repoActionsBulk');

    const [gooseRuns, branchRuns, idleScope] = await Promise.all([
      bulkRunningForScope('repo:block/goose'),
      bulkRunningForScope('branch-1'),
      bulkRunningForScope('repo:block/builderbot:apps/staged'),
    ]);

    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(invokeCommand).toHaveBeenCalledWith('get_all_running_actions');
    expect(gooseRuns.map((s) => s.executionId)).toEqual(['repo-run', 'repo-test']);
    expect(gooseRuns[0].phase).toEqual(phase);
    expect(gooseRuns[1].phase).toBeNull();
    // Branch-scoped executions come back in the same call and stay separate.
    expect(branchRuns.map((s) => s.executionId)).toEqual(['branch-run']);
    expect(idleScope).toEqual([]);
  });
});
