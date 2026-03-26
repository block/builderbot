import { describe, it, expect, vi, beforeEach } from 'vitest';
import { api, API_BASE } from './api';

const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

function jsonResponse(data: unknown, status = 200) {
  return Promise.resolve({
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(data),
    text: () => Promise.resolve(JSON.stringify(data)),
  });
}

async function loadFreshAPI() {
  vi.resetModules();
  return import('./api');
}

beforeEach(() => {
  mockFetch.mockReset();
  window.sessionStorage.clear();
});

describe('api', () => {
  it('listProjects calls GET /api/projects', async () => {
    const projects = [{ name: 'test', qualifiedName: 'ws/test' }];
    mockFetch.mockReturnValueOnce(jsonResponse(projects));

    const result = await api.listProjects();

    expect(mockFetch).toHaveBeenCalledWith(
      `${API_BASE}/api/projects`,
      expect.objectContaining({ headers: expect.objectContaining({ 'Content-Type': 'application/json' }) }),
    );
    expect(result).toEqual(projects);
  });

  it('getProjectFiles passes qualified name as path segments', async () => {
    mockFetch.mockReturnValueOnce(jsonResponse([]));
    await api.getProjectFiles('ws/my project');
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining('/api/project/ws/my project'),
      expect.anything(),
    );
  });

  it('createThread sends POST with body', async () => {
    const thread = { id: 't1', status: 'open' };
    mockFetch.mockReturnValueOnce(jsonResponse(thread));

    const req = {
      project: 'p',
      path: 'f.md',
      anchor: { selectedText: 'hello' },
      author: 'user',
      role: 'human',
      body: 'comment',
    };
    const result = await api.createThread(req);

    expect(mockFetch).toHaveBeenCalledWith(
      `${API_BASE}/api/threads`,
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify(req),
      }),
    );
    expect(result).toEqual(thread);
  });

  it('throws on non-ok response', async () => {
    mockFetch.mockReturnValueOnce(jsonResponse(null, 404));
    await expect(api.listProjects()).rejects.toThrow('API error: 404');
  });

  it('getRawFile returns text', async () => {
    mockFetch.mockReturnValueOnce(
      Promise.resolve({ ok: true, status: 200, text: () => Promise.resolve('# Hello') }),
    );
    const text = await api.getRawFile('proj', 'file.md');
    expect(text).toBe('# Hello');
  });

  it('patchThread sends PATCH', async () => {
    mockFetch.mockReturnValueOnce(jsonResponse({ ok: true }));
    await api.patchThread('t1', { project: 'p', path: 'f.md', status: 'resolved' });
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining('/api/threads/t1'),
      expect.objectContaining({ method: 'PATCH' }),
    );
  });

  it('checkInstallStatus calls GET /api/install-tools', async () => {
    const status = { cli: { installed: false }, plugin: { installed: false } };
    mockFetch.mockReturnValueOnce(jsonResponse(status));

    const result = await api.checkInstallStatus();

    expect(mockFetch).toHaveBeenCalledWith(
      `${API_BASE}/api/install-tools`,
      expect.objectContaining({ headers: expect.objectContaining({ 'Content-Type': 'application/json' }) }),
    );
    expect(result).toEqual(status);
  });

  it('installTools calls POST /api/install-tools', async () => {
    const status = { cli: { installed: true, path: '/usr/local/bin/penpal' }, plugin: { installed: true } };
    mockFetch.mockReturnValueOnce(jsonResponse(status));

    const result = await api.installTools();

    expect(mockFetch).toHaveBeenCalledWith(
      `${API_BASE}/api/install-tools`,
      expect.objectContaining({ method: 'POST' }),
    );
    expect(result).toEqual(status);
  });

  it('falls back to a unique in-memory window ID if sessionStorage is unavailable', async () => {
    const { api: freshAPI } = await loadFreshAPI();
    const getItemSpy = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (this: Storage, key: string) {
      if (this === window.sessionStorage) throw new Error('sessionStorage unavailable');
      return key ? null : null;
    });
    const setItemSpy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage) {
      if (this === window.sessionStorage) throw new Error('sessionStorage unavailable');
    });
    const uniqueWindowID = '11111111-1111-4111-8111-111111111111';
    const randomUUIDSpy = vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(uniqueWindowID);

    mockFetch
      .mockReturnValueOnce(jsonResponse({ ok: true }))
      .mockReturnValueOnce(jsonResponse({ ok: true }))
      .mockReturnValueOnce(jsonResponse({ ok: true }));

    await freshAPI.focusProject('ws/proj');
    await freshAPI.focusFile('ws/proj', 'thoughts/plan.md');
    await freshAPI.clearFocus();

    const urls = mockFetch.mock.calls.map(([url]) => new URL(String(url), 'http://localhost'));
    const windowIDs = urls.map((url) => url.searchParams.get('window'));

    expect(windowIDs).toEqual([uniqueWindowID, uniqueWindowID, uniqueWindowID]);

    randomUUIDSpy.mockRestore();
    setItemSpy.mockRestore();
    getItemSpy.mockRestore();
  });

  it('uses a stable per-window focus ID for focus endpoints', async () => {
    const { api: freshAPI } = await loadFreshAPI();
    mockFetch
      .mockReturnValueOnce(jsonResponse({ ok: true }))
      .mockReturnValueOnce(jsonResponse({ ok: true }))
      .mockReturnValueOnce(jsonResponse({ ok: true }));

    await freshAPI.focusProject('ws/proj');
    await freshAPI.focusFile('ws/proj', 'thoughts/plan.md', 'wt1');
    await freshAPI.clearFocus();

    const urls = mockFetch.mock.calls.map(([url]) => new URL(String(url), 'http://localhost'));
    const windowIDs = urls.map((url) => url.searchParams.get('window'));

    expect(windowIDs[0]).toBeTruthy();
    expect(windowIDs[1]).toBe(windowIDs[0]);
    expect(windowIDs[2]).toBe(windowIDs[0]);
    expect(urls[0].searchParams.get('project')).toBe('ws/proj');
    expect(urls[1].searchParams.get('path')).toBe('thoughts/plan.md');
    expect(urls[1].searchParams.get('worktree')).toBe('wt1');
  });
});
