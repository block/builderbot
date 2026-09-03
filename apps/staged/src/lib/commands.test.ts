import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

describe('browser-native command wrappers', () => {
  function selectedAcpConfig() {
    return {
      model: { configId: 'model', valueId: 'opus', label: 'Opus' },
      effort: { configId: 'reasoning_effort', valueId: 'high', label: 'High' },
    };
  }

  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.doUnmock('./transport');
    vi.unstubAllGlobals();
    vi.doUnmock('./transport');
    vi.doUnmock('./cache');
  });

  it('opens URLs with browser navigation in web mode', async () => {
    const opened = { opener: {} } as Window;
    const open = vi.fn(() => opened);
    const assign = vi.fn();
    vi.stubGlobal('window', { open, location: { assign } });

    const { openUrl } = await import('./commands');

    await openUrl('https://example.com/pull/1');

    expect(open).toHaveBeenCalledWith('https://example.com/pull/1', '_blank');
    expect(opened.opener).toBeNull();
    expect(assign).not.toHaveBeenCalled();
  });

  it('falls back to current-tab navigation when a new window cannot be opened', async () => {
    const open = vi.fn(() => null);
    const assign = vi.fn();
    vi.stubGlobal('window', { open, location: { assign } });

    const { openUrl } = await import('./commands');

    await openUrl('https://example.com/pull/2');

    expect(open).toHaveBeenCalledWith('https://example.com/pull/2', '_blank');
    expect(assign).toHaveBeenCalledWith('https://example.com/pull/2');
  });

  it('keeps path-based image uploads desktop-only in web mode', async () => {
    const fetch = vi.fn();
    vi.stubGlobal('fetch', fetch);

    const { createImage } = await import('./commands');

    await expect(createImage('branch-1', 'project-1', '/tmp/image.png')).rejects.toThrow(
      'desktop file paths'
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it('keeps opener discovery desktop-only in web mode', async () => {
    const fetch = vi.fn();
    vi.stubGlobal('window', {});
    vi.stubGlobal('fetch', fetch);

    const { getAvailableOpeners, openInApp } = await import('./features/branches/branch');

    await expect(getAvailableOpeners()).resolves.toEqual([]);
    await expect(openInApp('/tmp/repo', 'finder')).rejects.toThrow('web mode');
    expect(fetch).not.toHaveBeenCalled();
  });

  it('builds note follow-up prompts through the backend command', async () => {
    const invokeCommand = vi.fn().mockResolvedValue('backend prompt');
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { buildNoteFollowupMessage } = await import('./commands');

    await expect(buildNoteFollowupMessage('session-1', 'branch-1', true)).resolves.toBe(
      'backend prompt'
    );
    expect(invokeCommand).toHaveBeenCalledWith('build_note_followup_message', {
      sessionId: 'session-1',
      branchId: 'branch-1',
      hasParsedNote: true,
    });
  });

  it('routes queued follow-up message operations through backend commands', async () => {
    const queued = { id: 'queue-1', sessionId: 'session-1', content: 'next' };
    const invokeCommand = vi.fn().mockResolvedValue(queued);
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const {
      queueSessionMessage,
      listQueuedSessionMessages,
      deleteQueuedSessionMessage,
      sendQueuedSessionMessage,
    } = await import('./commands');

    await expect(queueSessionMessage('session-1', 'next', ['image-1'], 'branch-1')).resolves.toBe(
      queued
    );
    await listQueuedSessionMessages('session-1');
    await deleteQueuedSessionMessage('queue-1');
    await sendQueuedSessionMessage('queue-1');

    expect(invokeCommand.mock.calls).toEqual([
      [
        'queue_session_message',
        {
          sessionId: 'session-1',
          content: 'next',
          imageIds: ['image-1'],
          branchId: 'branch-1',
        },
      ],
      ['list_queued_session_messages', { sessionId: 'session-1' }],
      ['delete_queued_session_message', { id: 'queue-1' }],
      ['send_queued_session_message', { id: 'queue-1' }],
    ]);
  });

  it('forwards provider and ACP config selection when starting standalone sessions', async () => {
    const invokeCommand = vi.fn().mockResolvedValue({ id: 'session-1' });
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { startSession } = await import('./commands');
    const acpConfigSelection = selectedAcpConfig();

    await startSession('Investigate', '/repo', 'codex', acpConfigSelection);

    expect(invokeCommand).toHaveBeenCalledWith('start_session', {
      prompt: 'Investigate',
      workingDir: '/repo',
      provider: 'codex',
      acpConfigSelection,
    });
  });

  it('forwards provider and ACP config selection when starting project sessions', async () => {
    const invokeCommand = vi.fn().mockResolvedValue({
      sessionId: 'session-1',
      noteId: 'note-1',
    });
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { startProjectSession } = await import('./commands');
    const acpConfigSelection = selectedAcpConfig();

    await startProjectSession(
      'project-1',
      'Plan the work',
      'codex',
      ['image-1'],
      acpConfigSelection
    );

    expect(invokeCommand).toHaveBeenCalledWith('start_project_session', {
      projectId: 'project-1',
      prompt: 'Plan the work',
      provider: 'codex',
      imageIds: ['image-1'],
      acpConfigSelection,
    });
  });

  it('forwards provider and ACP config selection when starting or queueing branch sessions', async () => {
    const invokeCommand = vi.fn().mockResolvedValue({
      sessionId: 'session-1',
      artifactId: 'commit-1',
      sessionStatus: 'running',
    });
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { startOrQueueBranchSession } = await import('./commands');
    const launchContext = {
      source: 'diff_viewer' as const,
      scope: 'commit' as const,
      commitSha: 'abc123',
      reviewId: 'review-1',
    };
    const acpConfigSelection = selectedAcpConfig();

    await startOrQueueBranchSession(
      'branch-1',
      'Fix the bug',
      'commit',
      'codex',
      ['image-1'],
      launchContext,
      acpConfigSelection
    );

    expect(invokeCommand).toHaveBeenCalledWith('start_or_queue_branch_session', {
      branchId: 'branch-1',
      prompt: 'Fix the bug',
      sessionType: 'commit',
      provider: 'codex',
      imageIds: ['image-1'],
      launchContext,
      acpConfigSelection,
    });
  });

  it('forwards provider and ACP config selection when explicitly queueing branch sessions', async () => {
    const invokeCommand = vi.fn().mockResolvedValue({
      sessionId: 'session-1',
      artifactId: 'note-1',
      sessionStatus: 'queued',
    });
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { queueBranchSession } = await import('./commands');
    const launchContext = {
      source: 'diff_viewer' as const,
      scope: 'branch' as const,
      commitSha: 'abc123',
    };
    const acpConfigSelection = selectedAcpConfig();

    await queueBranchSession(
      'branch-1',
      'Write a note',
      'note',
      'codex',
      ['image-1'],
      launchContext,
      acpConfigSelection
    );

    expect(invokeCommand).toHaveBeenCalledWith('queue_branch_session', {
      branchId: 'branch-1',
      prompt: 'Write a note',
      sessionType: 'note',
      provider: 'codex',
      imageIds: ['image-1'],
      launchContext,
      acpConfigSelection,
    });
  });

  it('surfaces the queued-vs-running status returned by rebase and squash', async () => {
    const invokeCommand = vi
      .fn()
      .mockResolvedValueOnce({ sessionId: 'session-1', sessionStatus: 'queued' })
      .mockResolvedValueOnce({ sessionId: 'session-2', sessionStatus: 'running' });
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { rebaseBranch, squashCommits } = await import('./commands');

    await expect(rebaseBranch('branch-1', 'codex', 'origin')).resolves.toEqual({
      sessionId: 'session-1',
      sessionStatus: 'queued',
    });
    await expect(squashCommits('branch-1', 'codex')).resolves.toEqual({
      sessionId: 'session-2',
      sessionStatus: 'running',
    });

    expect(invokeCommand.mock.calls).toEqual([
      ['rebase_branch', { branchId: 'branch-1', provider: 'codex', target: 'origin' }],
      ['squash_commits', { branchId: 'branch-1', provider: 'codex' }],
    ]);
  });

  it('surfaces the queued-vs-running status returned by push and force push', async () => {
    const invokeCommand = vi
      .fn()
      .mockResolvedValueOnce({ sessionId: 'session-1', sessionStatus: 'running' })
      .mockResolvedValueOnce({ sessionId: 'session-2', sessionStatus: 'queued' });
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { pushBranch } = await import('./commands');

    await expect(pushBranch('branch-1', 'codex', false)).resolves.toEqual({
      sessionId: 'session-1',
      sessionStatus: 'running',
    });
    await expect(pushBranch('branch-1', 'codex', true)).resolves.toEqual({
      sessionId: 'session-2',
      sessionStatus: 'queued',
    });

    expect(invokeCommand.mock.calls).toEqual([
      ['push_branch', { branchId: 'branch-1', provider: 'codex', force: false }],
      ['push_branch', { branchId: 'branch-1', provider: 'codex', force: true }],
    ]);
  });

  it('distinguishes an immediate pull from a queued one', async () => {
    const invokeCommand = vi.fn().mockResolvedValueOnce(null).mockResolvedValueOnce('session-1');
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { pullOrQueueBranch } = await import('./commands');

    // null: the branch was idle, so the pull already fast-forwarded.
    await expect(pullOrQueueBranch('branch-1')).resolves.toBeNull();
    await expect(pullOrQueueBranch('branch-1')).resolves.toBe('session-1');

    expect(invokeCommand.mock.calls).toEqual([
      ['pull_or_queue_branch', { branchId: 'branch-1' }],
      ['pull_or_queue_branch', { branchId: 'branch-1' }],
    ]);
  });

  it('forwards ACP config selection when resuming a session', async () => {
    const invokeCommand = vi.fn().mockResolvedValue(undefined);
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { resumeSession } = await import('./commands');
    const acpConfigSelection = selectedAcpConfig();

    await resumeSession('session-1', 'Continue', ['image-1'], 'branch-1', acpConfigSelection);

    expect(invokeCommand).toHaveBeenCalledWith('resume_session', {
      sessionId: 'session-1',
      prompt: 'Continue',
      imageIds: ['image-1'],
      branchId: 'branch-1',
      acpConfigSelection,
    });
  });

  it('drains queued sessions without overriding the queued session provider or config', async () => {
    const invokeCommand = vi.fn().mockResolvedValue(true);
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { drainQueuedSessions } = await import('./commands');

    await drainQueuedSessions('branch-1');

    expect(invokeCommand).toHaveBeenCalledWith('drain_queued_sessions', {
      branchId: 'branch-1',
      provider: null,
    });
  });
});

describe('cached mutation command wrappers', () => {
  function deferred() {
    let resolve!: () => void;
    const promise = new Promise<void>((res) => {
      resolve = res;
    });
    return { promise, resolve };
  }

  let invokeCommand: ReturnType<typeof vi.fn>;
  let cachedCommand: ReturnType<typeof vi.fn>;
  let invalidateCache: ReturnType<typeof vi.fn>;
  let invalidateCacheByCommand: ReturnType<typeof vi.fn>;
  let invalidateCacheByArgs: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    invokeCommand = vi.fn();
    cachedCommand = vi.fn();
    invalidateCache = vi.fn();
    invalidateCacheByCommand = vi.fn();
    invalidateCacheByArgs = vi.fn();

    vi.doMock('./transport', () => ({
      isTauri: false,
      invokeCommand,
    }));
    vi.doMock('./cache', () => ({
      CACHE_SCHEMA_VERSION: 1,
      cachedCommand,
      cachedInvoke: vi.fn(),
      invalidateCache,
      invalidateCacheByCommand,
      invalidateCacheByArgs,
    }));
  });

  afterEach(() => {
    vi.doUnmock('./transport');
    vi.doUnmock('./cache');
  });

  it('waits for repo list invalidation before resolving addProjectRepo', async () => {
    const repo = { id: 'repo-1' };
    const invalidated = deferred();
    invokeCommand.mockResolvedValue(repo);
    invalidateCache.mockReturnValue(invalidated.promise);

    const { addProjectRepo } = await import('./commands');

    let settled = false;
    const result = addProjectRepo('project-1', 'block/builderbot').then((value) => {
      settled = true;
      return value;
    });

    await Promise.resolve();
    await Promise.resolve();

    expect(invalidateCache).toHaveBeenCalledWith('list_project_repos', { projectId: 'project-1' });
    expect(settled).toBe(false);

    invalidated.resolve();

    await expect(result).resolves.toBe(repo);
  });

  it('waits for all project cache invalidations before resolving deleteProject', async () => {
    const projectsInvalidated = deferred();
    const branchesInvalidated = deferred();
    const reposInvalidated = deferred();
    invokeCommand.mockResolvedValue(undefined);
    invalidateCacheByCommand
      .mockReturnValueOnce(projectsInvalidated.promise)
      .mockReturnValueOnce(branchesInvalidated.promise)
      .mockReturnValueOnce(reposInvalidated.promise);

    const { deleteProject } = await import('./commands');

    let settled = false;
    const result = deleteProject('project-1').then(() => {
      settled = true;
    });

    await Promise.resolve();
    await Promise.resolve();

    expect(invalidateCacheByCommand.mock.calls).toEqual([
      ['list_projects'],
      ['list_branches_for_project'],
      ['list_project_repos'],
    ]);
    expect(settled).toBe(false);

    projectsInvalidated.resolve();
    branchesInvalidated.resolve();
    await Promise.resolve();
    expect(settled).toBe(false);

    reposInvalidated.resolve();

    await expect(result).resolves.toBeUndefined();
  });

  it('bypasses the SWR cache when fetching fresh session messages', async () => {
    const messages = [{ id: 1, sessionId: 'session-1', role: 'assistant', content: 'done' }];
    invokeCommand.mockResolvedValue(messages);

    const { getFreshSessionMessages } = await import('./commands');

    await expect(getFreshSessionMessages('session-1')).resolves.toBe(messages);
    expect(invokeCommand).toHaveBeenCalledWith('get_session_messages', {
      sessionId: 'session-1',
    });
    expect(cachedCommand).not.toHaveBeenCalled();
  });

  it('passes force through project list caches as bypassRead', async () => {
    const projects = [{ id: 'project-1' }];
    const branches = [{ id: 'branch-1' }];
    const repos = [{ id: 'repo-1' }];
    cachedCommand
      .mockResolvedValueOnce({ data: projects, revalidating: null })
      .mockResolvedValueOnce({ data: branches, revalidating: null })
      .mockResolvedValueOnce({ data: repos, revalidating: null });

    const { listProjects, listBranchesForProject, listProjectRepos } = await import('./commands');

    await expect(listProjects({ force: true })).resolves.toEqual({
      data: projects,
      revalidating: null,
    });
    await expect(listBranchesForProject('project-1', { force: true })).resolves.toEqual({
      data: branches,
      revalidating: null,
    });
    await expect(listProjectRepos('project-1', { force: true })).resolves.toEqual({
      data: repos,
      revalidating: null,
    });

    expect(cachedCommand.mock.calls).toEqual([
      ['list_projects', undefined, { ttl: 5 * 60_000, bypassRead: true }],
      [
        'list_branches_for_project',
        { projectId: 'project-1' },
        { ttl: 2 * 60_000, bypassRead: true },
      ],
      ['list_project_repos', { projectId: 'project-1' }, { ttl: 10 * 60_000, bypassRead: true }],
    ]);
  });

  it('uses the standard provider discovery cache by default', async () => {
    const providers = [{ id: 'goose', label: 'Goose' }];
    cachedCommand.mockResolvedValue({ data: providers, revalidating: null });

    const { discoverAcpProviders } = await import('./commands');

    await expect(discoverAcpProviders()).resolves.toEqual({
      data: providers,
      revalidating: null,
    });
    expect(cachedCommand).toHaveBeenCalledWith('discover_acp_providers', undefined, {
      ttl: 30 * 60_000,
    });
  });

  it('forces provider discovery revalidation without bypassing the cached value', async () => {
    const providers = [{ id: 'goose', label: 'Goose' }];
    const revalidating = Promise.resolve([{ id: 'codex', label: 'Codex' }]);
    cachedCommand.mockResolvedValue({ data: providers, revalidating });

    const { discoverAcpProviders } = await import('./commands');

    await expect(discoverAcpProviders({ force: true })).resolves.toEqual({
      data: providers,
      revalidating,
    });
    expect(cachedCommand).toHaveBeenCalledWith('discover_acp_providers', undefined, { ttl: 0 });
  });

  it('discovers ACP config through the backend without frontend caching', async () => {
    const config = {
      providerId: 'goose',
      model: null,
      effort: null,
    };
    invokeCommand.mockResolvedValue(config);

    const { discoverAcpConfig } = await import('./commands');

    await expect(discoverAcpConfig('goose', '/repo')).resolves.toEqual({
      data: config,
      revalidating: null,
    });
    await discoverAcpConfig('goose', '/other-repo');

    expect(invokeCommand).toHaveBeenNthCalledWith(1, 'discover_acp_config', {
      providerId: 'goose',
      workingDir: '/repo',
      force: false,
      selectedModelValue: null,
    });
    expect(invokeCommand).toHaveBeenNthCalledWith(2, 'discover_acp_config', {
      providerId: 'goose',
      workingDir: '/other-repo',
      force: false,
      selectedModelValue: null,
    });
    expect(cachedCommand).not.toHaveBeenCalledWith(
      'discover_acp_config',
      expect.anything(),
      expect.anything()
    );
  });

  it('passes ACP config discovery force to the backend command', async () => {
    const config = {
      providerId: 'goose',
      model: null,
      effort: null,
    };
    invokeCommand.mockResolvedValue(config);

    const { discoverAcpConfig } = await import('./commands');

    await expect(discoverAcpConfig('goose', null, { force: true })).resolves.toEqual({
      data: config,
      revalidating: null,
    });
    expect(invokeCommand).toHaveBeenCalledWith('discover_acp_config', {
      providerId: 'goose',
      workingDir: null,
      force: true,
      selectedModelValue: null,
    });
  });

  it('passes selected ACP model discovery through to the backend command', async () => {
    const config = {
      providerId: 'goose',
      model: null,
      effort: null,
    };
    invokeCommand.mockResolvedValue(config);

    const { discoverAcpConfig } = await import('./commands');

    await expect(
      discoverAcpConfig('goose', '/repo', { selectedModelValue: 'opus' })
    ).resolves.toEqual({
      data: config,
      revalidating: null,
    });
    expect(invokeCommand).toHaveBeenCalledWith('discover_acp_config', {
      providerId: 'goose',
      workingDir: '/repo',
      force: false,
      selectedModelValue: 'opus',
    });
  });
});

describe('timeline boot snapshot', () => {
  /** Deliberately not the real value — the snapshot must track whatever the shared constant says. */
  const SCHEMA_VERSION = 7;

  const stale = {
    commits: [{ sha: 'pending-1', subject: 'Rebase onto main', pipelineKind: 'rebase' }],
    notes: [],
    reviews: [],
    images: [],
  };
  const fetched = {
    commits: [{ sha: 'abc123', subject: 'Fix the thing' }],
    notes: [],
    reviews: [],
    images: [],
  };

  let storage: Record<string, string>;
  let cachedInvoke: ReturnType<typeof vi.fn>;
  let snapshotKey: string;

  beforeEach(async () => {
    vi.resetModules();
    storage = {};
    vi.stubGlobal('localStorage', {
      getItem: (key: string) => storage[key] ?? null,
      setItem: (key: string, value: string) => {
        storage[key] = value;
      },
      removeItem: (key: string) => {
        delete storage[key];
      },
    });
    cachedInvoke = vi.fn(async function* () {
      yield { data: fetched, source: 'network', fetchedAt: Date.now() };
    });

    vi.doMock('./transport', () => ({ isTauri: false, invokeCommand: vi.fn() }));
    vi.doMock('./cache', () => ({
      CACHE_SCHEMA_VERSION: SCHEMA_VERSION,
      cachedCommand: vi.fn(),
      cachedInvoke,
      invalidateCache: vi.fn(),
      invalidateCacheByCommand: vi.fn(),
      invalidateCacheByArgs: vi.fn(),
    }));

    ({
      SNAPSHOT_KEYS: { timelines: snapshotKey },
    } = await import('./shared/webSnapshot'));
  });

  afterEach(() => {
    vi.doUnmock('./transport');
    vi.doUnmock('./cache');
    vi.unstubAllGlobals();
  });

  it('discards a snapshot written before the payload carried a version', async () => {
    // Pre-versioning format: a bare branchId -> entry record.
    storage[snapshotKey] = JSON.stringify({
      'branch-1': { timeline: stale, fetchedAt: Date.now() },
    });

    const { getBranchTimelineWithRevalidation } = await import('./commands');
    const { cached, fresh } = getBranchTimelineWithRevalidation('branch-1');

    expect(cached).toBeNull();
    await expect(fresh).resolves.toEqual(fetched);
    expect(storage[snapshotKey]).toBeUndefined();
  });

  it('discards a snapshot stamped by a previous build', async () => {
    storage[snapshotKey] = JSON.stringify({
      schemaVersion: SCHEMA_VERSION - 1,
      timelines: { 'branch-1': { timeline: stale, fetchedAt: Date.now() } },
    });

    const { getBranchTimelineWithRevalidation } = await import('./commands');
    const { cached, fresh } = getBranchTimelineWithRevalidation('branch-1');

    expect(cached).toBeNull();
    await expect(fresh).resolves.toEqual(fetched);
    expect(storage[snapshotKey]).toBeUndefined();
  });

  it('round-trips the timeline cache through a snapshot on the current version', async () => {
    const beforeReload = await import('./commands');
    await expect(beforeReload.getBranchTimeline('branch-1')).resolves.toEqual(fetched);
    beforeReload.persistTimelineSnapshot();

    expect(JSON.parse(storage[snapshotKey])).toEqual({
      schemaVersion: SCHEMA_VERSION,
      timelines: { 'branch-1': { timeline: fetched, fetchedAt: expect.any(Number) } },
    });

    cachedInvoke.mockClear();
    vi.resetModules();
    const afterReload = await import('./commands');

    // A just-seeded entry is inside TIMELINE_FRESH_MS, so this read is served
    // with no fetch behind it — the window a mismatched payload shape would
    // otherwise reach the UI through uncorrected.
    expect(afterReload.getBranchTimelineWithRevalidation('branch-1')).toEqual({
      cached: fetched,
      fresh: null,
    });
    expect(cachedInvoke).not.toHaveBeenCalled();
  });
});
