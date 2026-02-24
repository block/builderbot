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

  it('getProjectFiles encodes qualified name', async () => {
    mockFetch.mockReturnValueOnce(jsonResponse([]));
    await api.getProjectFiles('ws/my project');
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining('/api/project/ws%2Fmy%20project'),
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
});
