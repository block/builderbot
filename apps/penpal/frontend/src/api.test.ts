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

  it('uses a stable per-window focus ID for focus endpoints', async () => {
    mockFetch
      .mockReturnValueOnce(jsonResponse({ ok: true }))
      .mockReturnValueOnce(jsonResponse({ ok: true }))
      .mockReturnValueOnce(jsonResponse({ ok: true }));

    await api.focusProject('ws/proj');
    await api.focusFile('ws/proj', 'thoughts/plan.md', 'wt1');
    await api.clearFocus();

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
