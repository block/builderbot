import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

describe('browser-native command wrappers', () => {
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

  beforeEach(() => {
    vi.resetModules();
    invokeCommand = vi.fn();
    cachedCommand = vi.fn();
    invalidateCache = vi.fn();
    invalidateCacheByCommand = vi.fn();

    vi.doMock('./transport', () => ({
      isTauri: false,
      invokeCommand,
    }));
    vi.doMock('./cache', () => ({
      cachedCommand,
      cachedInvoke: vi.fn(),
      invalidateCache,
      invalidateCacheByCommand,
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
});
