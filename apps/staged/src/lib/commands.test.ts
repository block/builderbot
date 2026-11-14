import 'fake-indexeddb/auto';
import { IDBFactory } from 'fake-indexeddb';
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { createStore, keys as idbKeys } from 'idb-keyval';

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

  it('surfaces the queued-vs-running status returned by rebase and squash, and the revert session', async () => {
    const invokeCommand = vi
      .fn()
      .mockResolvedValueOnce({ sessionId: 'session-1', sessionStatus: 'queued' })
      .mockResolvedValueOnce({ sessionId: 'session-2', sessionStatus: 'running' })
      .mockResolvedValueOnce({
        sessionId: 'session-3',
        sessionStatus: 'running',
        followUpSessionId: 'session-4',
      })
      .mockResolvedValueOnce('session-5');
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { rebaseBranch, squashCommits, revertCommit } = await import('./commands');

    await expect(rebaseBranch('branch-1', 'codex', 'origin')).resolves.toEqual({
      sessionId: 'session-1',
      sessionStatus: 'queued',
    });
    await expect(squashCommits('branch-1', 'codex')).resolves.toEqual({
      sessionId: 'session-2',
      sessionStatus: 'running',
    });
    await expect(rebaseBranch('branch-1', 'codex', undefined, true)).resolves.toEqual({
      sessionId: 'session-3',
      sessionStatus: 'running',
      followUpSessionId: 'session-4',
    });
    await expect(revertCommit('branch-1', 'abc1234')).resolves.toBe('session-5');

    expect(invokeCommand.mock.calls).toEqual([
      [
        'rebase_branch',
        { branchId: 'branch-1', provider: 'codex', target: 'origin', thenForcePush: null },
      ],
      ['squash_commits', { branchId: 'branch-1', provider: 'codex' }],
      [
        'rebase_branch',
        { branchId: 'branch-1', provider: 'codex', target: null, thenForcePush: true },
      ],
      ['revert_commit', { branchId: 'branch-1', commitSha: 'abc1234', provider: null }],
    ]);
  });

  it('surfaces the queued-vs-running status returned by push and force push', async () => {
    const invokeCommand = vi
      .fn()
      .mockResolvedValueOnce({ sessionId: 'session-1', sessionStatus: 'running' })
      .mockResolvedValueOnce({ sessionId: 'session-2', sessionStatus: 'queued' })
      .mockResolvedValueOnce({ sessionId: 'session-3', sessionStatus: 'running' });
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
    await expect(pushBranch('branch-1', 'codex', false, true)).resolves.toEqual({
      sessionId: 'session-3',
      sessionStatus: 'running',
    });

    expect(invokeCommand.mock.calls).toEqual([
      ['push_branch', { branchId: 'branch-1', provider: 'codex', force: false, noVerify: null }],
      ['push_branch', { branchId: 'branch-1', provider: 'codex', force: true, noVerify: null }],
      ['push_branch', { branchId: 'branch-1', provider: 'codex', force: false, noVerify: true }],
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

  it('starts a queued session now without overriding the queued session provider or config', async () => {
    const invokeCommand = vi.fn().mockResolvedValue(true);
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { startQueuedSessionNow } = await import('./commands');

    await startQueuedSessionNow('branch-1', 'session-1');

    expect(invokeCommand).toHaveBeenCalledWith('start_queued_session_now', {
      branchId: 'branch-1',
      sessionId: 'session-1',
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

describe('persisted branch timelines (desktop)', () => {
  type CommandsModule = typeof import('./commands');

  const timeline = (sha: string) => ({ commits: [{ sha }], notes: [], reviews: [], images: [] });
  const branch = (id: string, over: Record<string, unknown> = {}) => ({
    id,
    projectId: 'p1',
    projectRepoId: null,
    branchType: 'local',
    branchName: id,
    baseBranch: 'main',
    workspaceName: null,
    worktreePath: `/wt/${id}`,
    ...over,
  });
  const store = () => createStore('staged-cache', 'responses');

  let invoke: ReturnType<typeof vi.fn>;
  /** `invoke` answers from here, so a test can change a list between launches. */
  let backend: {
    branches: Record<string, unknown[]>;
    projects: unknown[];
    timelines: Record<string, unknown>;
  };

  function respond(command: string, args: Record<string, string>) {
    switch (command) {
      case 'list_projects':
        return backend.projects;
      case 'list_branches_for_project':
        return backend.branches[args.projectId] ?? [];
      case 'get_branch_timeline':
        return backend.timelines[args.branchId];
      default:
        throw new Error(`unexpected command ${command}`);
    }
  }

  /** Fresh module graph in desktop mode over the same IndexedDB — a relaunch. */
  async function launch({ brokenIdb = false, awaitSeed = true } = {}): Promise<CommandsModule> {
    vi.resetModules();
    // Bound per launch: a previous instance finishing an IndexedDB read late
    // must call its own spy, not the one the current launch is asserting on.
    const spy = vi.fn(async (command: string, args: Record<string, string>) =>
      respond(command, args)
    );
    invoke = spy;
    vi.doMock('./transport', () => ({
      isTauri: true,
      invokeCommand: (...args: [string, Record<string, string>]) => spy(...args),
    }));
    if (brokenIdb) {
      vi.doMock('idb-keyval', async (importOriginal) => {
        const actual = await importOriginal<typeof import('idb-keyval')>();
        const broken = () => Promise.reject(new Error('IndexedDB unavailable'));
        return { ...actual, createStore: () => broken, get: broken, set: broken, del: broken };
      });
    }
    const commands = await import('./commands');
    if (awaitSeed) await commands.whenTimelineCacheSeeded();
    return commands;
  }

  /** A first session that lists `p1`, fetches every branch's timeline and so persists them. */
  async function firstSession(): Promise<void> {
    vi.setSystemTime(1_000_000);
    const commands = await launch();
    await commands.listBranchesForProject('p1');
    for (const b of backend.branches.p1 as Array<{ id: string }>) {
      await commands.getBranchTimeline(b.id);
    }
    // Relaunches happen a minute later, well past TIMELINE_FRESH_MS.
    vi.setSystemTime(1_060_000);
  }

  const timelineCalls = () =>
    invoke.mock.calls.filter(([command]) => command === 'get_branch_timeline');

  beforeEach(() => {
    // A new IndexedDB per test, so nothing persists across tests — only across launches.
    vi.stubGlobal('indexedDB', new IDBFactory());
    vi.useFakeTimers({ toFake: ['Date'] });
    backend = {
      projects: [{ id: 'p1' }],
      branches: { p1: [branch('b1'), branch('b2')] },
      timelines: { b1: timeline('b1-v1'), b2: timeline('b2-v1') },
    };
  });

  afterEach(() => {
    vi.doUnmock('./transport');
    vi.doUnmock('idb-keyval');
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('paints the last persisted timeline on launch and issues one forced refetch per card', async () => {
    await firstSession();
    backend.timelines = { b1: timeline('b1-v2'), b2: timeline('b2-v2') };

    const commands = await launch();
    const first = commands.getBranchTimelineWithRevalidation('b1');
    const second = commands.getBranchTimelineWithRevalidation('b2');

    // Stale paint with the original fetchedAt behind it, refetch already in flight.
    expect(first.cached).toEqual(timeline('b1-v1'));
    expect(second.cached).toEqual(timeline('b2-v1'));
    expect(first.fresh).not.toBeNull();
    expect(timelineCalls()).toEqual([
      ['get_branch_timeline', { branchId: 'b1' }],
      ['get_branch_timeline', { branchId: 'b2' }],
    ]);

    await expect(first.fresh).resolves.toEqual(timeline('b1-v2'));
    await expect(second.fresh).resolves.toEqual(timeline('b2-v2'));
    // The fresh result replaces the paint and is itself now fresh — no second refetch.
    expect(commands.getBranchTimelineWithRevalidation('b1')).toEqual({
      cached: timeline('b1-v2'),
      fresh: null,
    });
    expect(timelineCalls()).toHaveLength(2);

    // ...and it is what the next launch will paint.
    const next = await launch();
    expect(next.getBranchTimelineWithRevalidation('b1').cached).toEqual(timeline('b1-v2'));
  });

  it('refetches a seeded entry even when its fetchedAt is within TIMELINE_FRESH_MS', async () => {
    const commands0 = await launch();
    await commands0.listBranchesForProject('p1');
    await commands0.getBranchTimeline('b1');
    backend.timelines = { b1: timeline('b1-v2'), b2: timeline('b2-v1') };

    // Relaunch with the clock untouched: the seeded entry's fetchedAt is "now".
    const commands = await launch();
    const { cached, fresh } = commands.getBranchTimelineWithRevalidation('b1');
    expect(cached).toEqual(timeline('b1-v1'));
    expect(fresh).not.toBeNull();
    await expect(fresh).resolves.toEqual(timeline('b1-v2'));
    expect(timelineCalls()).toEqual([['get_branch_timeline', { branchId: 'b1' }]]);

    // Once this launch's result is in, it is fresh and nothing more is fetched.
    expect(commands.getBranchTimelineWithRevalidation('b1')).toEqual({
      cached: timeline('b1-v2'),
      fresh: null,
    });
    expect(timelineCalls()).toHaveLength(1);
  });

  it('misses as before when nothing is persisted, then persists the result', async () => {
    const commands = await launch();
    await commands.listBranchesForProject('p1');

    const { cached, fresh } = commands.getBranchTimelineWithRevalidation('b1');
    expect(cached).toBeNull();
    await expect(fresh).resolves.toEqual(timeline('b1-v1'));
    expect(timelineCalls()).toHaveLength(1);
    await expect(idbKeys(store())).resolves.toEqual([
      expect.stringContaining('get_branch_timeline:'),
    ]);
  });

  it('falls back to a plain fetch when IndexedDB is unavailable', async () => {
    const commands = await launch({ brokenIdb: true });
    await commands.listBranchesForProject('p1');

    const { cached, fresh } = commands.getBranchTimelineWithRevalidation('b1');
    expect(cached).toBeNull();
    await expect(fresh).resolves.toEqual(timeline('b1-v1'));
    expect(timelineCalls()).toHaveLength(1);
  });

  it('drops a seeded entry once the branch list no longer has its branch or identity', async () => {
    await firstSession();
    // b2 was deleted; b1 now lives in a different worktree.
    backend.branches.p1 = [branch('b1', { worktreePath: '/wt/elsewhere' })];

    const commands = await launch();
    expect(commands.getBranchTimelineWithRevalidation('b2').cached).toEqual(timeline('b2-v1'));

    await commands.listBranchesForProject('p1');
    expect(commands.getBranchTimelineWithRevalidation('b1').cached).toBeNull();
    expect(commands.getBranchTimelineWithRevalidation('b2').cached).toBeNull();
    await vi.waitFor(async () => {
      expect(await idbKeys(store())).toEqual([]);
    });
  });

  it('drops seeded entries whose project is gone', async () => {
    await firstSession();
    backend.projects = [{ id: 'p-other' }];

    const commands = await launch();
    await commands.listProjects();

    expect(commands.getBranchTimelineWithRevalidation('b1').cached).toBeNull();
    expect(commands.getBranchTimelineWithRevalidation('b2').cached).toBeNull();
    await vi.waitFor(async () => {
      expect(await idbKeys(store())).toEqual([]);
    });
  });

  it('honours a branch list that lands before the seed settles', async () => {
    await firstSession();
    backend.branches.p1 = [branch('b1')];

    const commands = await launch({ awaitSeed: false });
    // The backend answers on a microtask; the IndexedDB read takes longer.
    await commands.listBranchesForProject('p1');
    await commands.whenTimelineCacheSeeded();

    expect(commands.getBranchTimelineWithRevalidation('b1').cached).toEqual(timeline('b1-v1'));
    expect(commands.getBranchTimelineWithRevalidation('b2').cached).toBeNull();
  });

  it('treats a seeded entry as a miss for plain reads, which join the card refetch', async () => {
    await firstSession();
    backend.timelines = { b1: timeline('b1-v2'), b2: timeline('b2-v2') };

    const commands = await launch();
    // A non-card caller (hashtag items, the diff session launcher) asks plainly:
    // it gets this launch's timeline, never the persisted one, from one fetch.
    await expect(commands.getBranchTimeline('b1')).resolves.toEqual(timeline('b1-v2'));
    expect(timelineCalls()).toEqual([['get_branch_timeline', { branchId: 'b1' }]]);
    // That fetch replaced the seeded entry: the card is fresh and plain reads
    // are answered from memory again.
    expect(commands.getBranchTimelineWithRevalidation('b1')).toEqual({
      cached: timeline('b1-v2'),
      fresh: null,
    });
    await expect(commands.getBranchTimeline('b1')).resolves.toEqual(timeline('b1-v2'));
    expect(timelineCalls()).toHaveLength(1);

    // A card that painted first: the plain read shares its forced refetch.
    const card = commands.getBranchTimelineWithRevalidation('b2');
    expect(card.cached).toEqual(timeline('b2-v1'));
    await expect(commands.getBranchTimeline('b2')).resolves.toEqual(timeline('b2-v2'));
    await expect(card.fresh).resolves.toEqual(timeline('b2-v2'));
    expect(timelineCalls()).toHaveLength(2);
  });

  it('seeds nothing older than a day and removes it from IndexedDB', async () => {
    await firstSession();
    const day = 24 * 60 * 60 * 1000;

    // Just inside the cutoff still paints; its refetch re-stamps b1 only.
    vi.setSystemTime(1_000_000 + day - 60_000);
    const first = await launch();
    const b1 = first.getBranchTimelineWithRevalidation('b1');
    expect(b1.cached).toEqual(timeline('b1-v1'));
    await b1.fresh;

    // Past the cutoff: b2 (still stamped from the first session) is dropped
    // from the paint and the store; b1, re-fetched a minute ago, is kept.
    vi.setSystemTime(1_000_000 + day + 60_000);
    const second = await launch();
    expect(second.getBranchTimelineWithRevalidation('b2').cached).toBeNull();
    expect(second.getBranchTimelineWithRevalidation('b1').cached).toEqual(timeline('b1-v1'));
    await vi.waitFor(async () => {
      expect(await idbKeys(store())).toEqual([expect.stringContaining('b1')]);
    });
  });

  it('never rolls back a timeline a card fetched before the seed landed', async () => {
    await firstSession();
    backend.timelines.b1 = timeline('b1-v2');

    const commands = await launch({ awaitSeed: false });
    const fetched = commands.getBranchTimeline('b1', { force: true });
    await commands.whenTimelineCacheSeeded();
    await fetched;

    expect(commands.getBranchTimelineWithRevalidation('b1').cached).toEqual(timeline('b1-v2'));
  });
});

/**
 * `update_note`, `create_note`, `set_note_read`, `mark_session_chat_read`
 * and `queue_session_message` run on the backend's thread pool, where two in
 * flight land in either order. Each transport call here is settled by hand,
 * so a test can leave the first in flight and see whether the second is sent.
 */
describe('writes sent without awaiting keep their send order', () => {
  type Sent = { command: string; args: Record<string, unknown>; settle: (value?: unknown) => void };

  async function load() {
    vi.resetModules();
    const sent: Sent[] = [];
    const invokeCommand = vi.fn(
      (command: string, args: Record<string, unknown>) =>
        new Promise((resolve) => {
          sent.push({ command, args, settle: resolve });
        })
    );
    vi.doMock('./transport', () => ({ invokeCommand, isTauri: true }));
    const commands = await import('./commands');
    return { commands, sent };
  }

  async function flush(): Promise<void> {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  afterEach(() => {
    vi.doUnmock('./transport');
  });

  it('sends a note’s second save only once the first has landed', async () => {
    const { commands, sent } = await load();

    void commands.updateNote('note-1', 'Plan', 'older');
    void commands.updateNote('note-1', 'Plan', 'newer');
    await flush();
    expect(sent.map((call) => call.args.content)).toEqual(['older']);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.args.content)).toEqual(['older', 'newer']);
  });

  it('sends the read mark only once the unread mark before it has landed', async () => {
    const { commands, sent } = await load();

    void commands.setNoteRead('note-1', false);
    void commands.setNoteRead('note-1', true);
    await flush();
    expect(sent.map((call) => call.args.read)).toEqual([false]);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.args.read)).toEqual([false, true]);
  });

  it('orders a note’s save and its read mark against each other', async () => {
    const { commands, sent } = await load();

    void commands.setNoteRead('note-1', true);
    void commands.updateNote('note-1', 'Plan', 'body');
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['set_note_read']);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['set_note_read', 'update_note']);
  });

  it('sends a note’s delete only once the unread mark before it has landed', async () => {
    const { commands, sent } = await load();

    void commands.setNoteRead('note-1', false);
    void commands.deleteNote('note-1');
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['set_note_read']);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['set_note_read', 'delete_note']);
  });

  it('creates a branch’s dropped notes one at a time, in drop order', async () => {
    const { commands, sent } = await load();

    void commands.createNote('branch-1', 'a.md', 'A');
    void commands.createNote('branch-1', 'b.md', 'B');
    void commands.createNote('branch-1', 'c.md', 'C');
    await flush();
    expect(sent.map((call) => call.args.title)).toEqual(['a.md']);

    sent[0].settle({});
    await flush();
    sent[1].settle({});
    await flush();
    expect(sent.map((call) => call.args.title)).toEqual(['a.md', 'b.md', 'c.md']);
  });

  it('queues a session’s second message only once the first is queued', async () => {
    const { commands, sent } = await load();

    void commands.queueSessionMessage('session-1', 'first');
    void commands.queueSessionMessage('session-1', 'second');
    await flush();
    expect(sent.map((call) => call.args.content)).toEqual(['first']);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.args.content)).toEqual(['first', 'second']);
  });

  it('sends a chat’s close mark only once its open mark has landed', async () => {
    const { commands, sent } = await load();

    void commands.markSessionChatRead('session-1');
    void commands.markSessionChatRead('session-1');
    await flush();
    expect(sent).toHaveLength(1);

    sent[0].settle();
    await flush();
    expect(sent).toHaveLength(2);
  });

  it('sends a repo badge’s second save only once the first has landed', async () => {
    const { commands, sent } = await load();

    void commands.updateRepoBadge('acme/app', '', 'AP', 10);
    void commands.updateRepoBadge('acme/app', '', 'APP', 10);
    await flush();
    expect(sent.map((call) => call.args.shortName)).toEqual(['AP']);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.args.shortName)).toEqual(['AP', 'APP']);
  });

  it('sends a repo badge’s delete only once the save before it has landed', async () => {
    const { commands, sent } = await load();

    void commands.updateRepoBadge('acme/app', 'web', 'AP', 10);
    void commands.deleteRepoBadge('acme/app', 'web');
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['update_repo_badge']);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['update_repo_badge', 'delete_repo_badge']);
  });

  it('sends pinned repo reorders one at a time, in drop order', async () => {
    const { commands, sent } = await load();

    void commands.reorderPinnedRepos([
      ['acme/a', ''],
      ['acme/b', ''],
    ]);
    void commands.reorderPinnedRepos([
      ['acme/b', ''],
      ['acme/a', ''],
    ]);
    await flush();
    expect(sent).toHaveLength(1);

    sent[0].settle();
    await flush();
    expect(sent.map((call) => (call.args.orderedKeys as [string, string][])[0][0])).toEqual([
      'acme/a',
      'acme/b',
    ]);
  });

  it('sends a project note’s read mark only once the unread mark before it has landed', async () => {
    const { commands, sent } = await load();

    void commands.setProjectNoteRead('pnote-1', false);
    void commands.setProjectNoteRead('pnote-1', true);
    await flush();
    expect(sent.map((call) => call.args.read)).toEqual([false]);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.args.read)).toEqual([false, true]);
  });

  it('sends a project note’s delete only once the unread mark before it has landed', async () => {
    const { commands, sent } = await load();

    void commands.setProjectNoteRead('pnote-1', false);
    void commands.deleteProjectNote('pnote-1');
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['set_project_note_read']);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.command)).toEqual([
      'set_project_note_read',
      'delete_project_note',
    ]);
  });

  it('sends a tag’s catalog edit only once the assignment before it has landed', async () => {
    const { commands, sent } = await load();

    void commands.setProjectTagAssigned('project-1', 'tag-1', true);
    void commands.moveTag('tag-1', 'group-1', 0);
    void commands.setProjectGroupTag('project-2', 'group-1', 'tag-2');
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['set_project_tag_assigned']);

    sent[0].settle();
    await flush();
    expect(sent.map((call) => call.command)).toEqual(['set_project_tag_assigned', 'move_tag']);

    sent[1].settle();
    await flush();
    expect(sent.map((call) => call.command)).toEqual([
      'set_project_tag_assigned',
      'move_tag',
      'set_project_group_tag',
    ]);
  });

  it('sends a project’s second rename only once the first has landed', async () => {
    const { commands, sent } = await load();

    void commands.renameProject('project-1', 'older');
    void commands.renameProject('project-1', 'newer');
    // Another project's rename waits for neither.
    void commands.renameProject('project-2', 'other');
    await flush();
    expect(sent.map((call) => call.args.name)).toEqual(['older', 'other']);

    sent[0].settle({});
    await flush();
    expect(sent.map((call) => call.args.name)).toEqual(['older', 'other', 'newer']);
  });

  it('sends the next write after one that failed', async () => {
    vi.resetModules();
    const invokeCommand = vi
      .fn()
      .mockRejectedValueOnce(new Error('Note not found: note-1'))
      .mockResolvedValueOnce({ id: 'note-1' });
    vi.doMock('./transport', () => ({ invokeCommand, isTauri: true }));
    const commands = await import('./commands');

    const failed = commands.updateNote('note-1', 'Plan', 'older');
    const next = commands.updateNote('note-1', 'Plan', 'newer');

    await expect(failed).rejects.toThrow('Note not found');
    await expect(next).resolves.toEqual({ id: 'note-1' });
  });

  it('does not hold a write back for another note, branch or session', async () => {
    const { commands, sent } = await load();

    void commands.updateNote('note-1', 'Plan', 'one');
    void commands.updateNote('note-2', 'Plan', 'two');
    void commands.setNoteRead('note-3', true);
    void commands.createNote('branch-1', 'a.md', 'A');
    void commands.createNote('branch-2', 'a.md', 'A');
    void commands.queueSessionMessage('session-1', 'first');
    void commands.queueSessionMessage('session-2', 'first');
    void commands.markSessionChatRead('session-1');
    void commands.updateRepoBadge('acme/app', '', 'AP', 10);
    void commands.updateRepoBadge('acme/app', 'web', 'WB', 10);
    void commands.setProjectNoteRead('pnote-1', true);
    void commands.setProjectNoteRead('pnote-2', true);
    void commands.reorderPinnedRepos([]);
    void commands.setProjectTagAssigned('project-1', 'tag-1', true);
    await flush();

    expect(sent).toHaveLength(14);
  });

  describe('when the write ahead never settles', () => {
    beforeEach(() => {
      // Only the timers the wait bound uses, so module loading is untouched.
      vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
      vi.spyOn(console, 'warn').mockImplementation(() => {});
    });

    afterEach(() => {
      vi.useRealTimers();
      vi.restoreAllMocks();
    });

    async function loadFor(transport: 'tauri' | 'web') {
      vi.resetModules();
      const sent: Sent[] = [];
      const invokeCommand = vi.fn(
        (command: string, args: Record<string, unknown>) =>
          new Promise((resolve) => {
            sent.push({ command, args, settle: resolve });
          })
      );
      vi.doMock('./transport', () => ({ invokeCommand, isTauri: transport === 'tauri' }));
      const commands = await import('./commands');
      return { commands, sent };
    }

    it('over HTTP, sends the next write for the key once the wait bound runs out', async () => {
      const { commands, sent } = await loadFor('web');

      // A request on a dropped connection: `fetch` has no timeout.
      const hung = commands.updateNote('note-1', 'Plan', 'older');
      void commands.updateNote('note-1', 'Plan', 'newer');
      await vi.advanceTimersByTimeAsync(commands.WEB_WRITE_ORDER_MAX_WAIT_MS - 1);
      expect(sent.map((call) => call.args.content)).toEqual(['older']);

      await vi.advanceTimersByTimeAsync(1);
      expect(sent.map((call) => call.args.content)).toEqual(['older', 'newer']);

      // The hung request was not failed or cancelled: it can still answer.
      sent[0].settle({ id: 'note-1' });
      await expect(hung).resolves.toEqual({ id: 'note-1' });
    });

    it('over HTTP, keeps the writes queued behind a hung one in the order they were sent', async () => {
      const { commands, sent } = await loadFor('web');

      void commands.updateNote('note-1', 'Plan', 'hung');
      void commands.updateNote('note-1', 'Plan', 'second');
      void commands.updateNote('note-1', 'Plan', 'third');
      await vi.advanceTimersByTimeAsync(commands.WEB_WRITE_ORDER_MAX_WAIT_MS);
      // The bound ran out for both at the same moment, but only the write
      // directly behind the hung one is sent beside it.
      expect(sent.map((call) => call.args.content)).toEqual(['hung', 'second']);

      sent[1].settle({});
      await vi.advanceTimersByTimeAsync(0);
      expect(sent.map((call) => call.args.content)).toEqual(['hung', 'second', 'third']);
    });

    it('under Tauri, keeps the next write back for as long as the one ahead is out', async () => {
      const { commands, sent } = await loadFor('tauri');

      void commands.updateNote('note-1', 'Plan', 'older');
      void commands.updateNote('note-1', 'Plan', 'newer');
      await vi.advanceTimersByTimeAsync(100 * commands.WEB_WRITE_ORDER_MAX_WAIT_MS);
      expect(sent.map((call) => call.args.content)).toEqual(['older']);

      sent[0].settle({});
      await vi.advanceTimersByTimeAsync(0);
      expect(sent.map((call) => call.args.content)).toEqual(['older', 'newer']);
    });
  });
});
