import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const transport = vi.hoisted(() => ({ isTauri: false }));
const commands = vi.hoisted(() => ({ getToolCallBody: vi.fn() }));

vi.mock('../../transport', () => transport);
vi.mock('../../commands', () => commands);

import { loadToolCallBodies, loadToolCallBody, resetToolCallBodyCache } from './toolBodySource';

const body = (messageId: number) => ({ messageId, acpRawOutput: { stdout: `out ${messageId}` } });

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

describe('tool body source', () => {
  const fetchMock = vi.fn<typeof fetch>();

  beforeEach(() => {
    transport.isTauri = false;
    commands.getToolCallBody.mockReset();
    fetchMock.mockReset();
    vi.stubGlobal('fetch', fetchMock);
    resetToolCallBodyCache();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('fetches /api/tool-bodies/<id> once and serves later requests from the cache', async () => {
    fetchMock.mockResolvedValue(jsonResponse(body(42)));

    await expect(loadToolCallBody(42)).resolves.toEqual(body(42));
    await expect(loadToolCallBody(42)).resolves.toEqual(body(42));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledWith('/api/tool-bodies/42');
    expect(commands.getToolCallBody).not.toHaveBeenCalled();
  });

  it('rejects on a non-OK response and forgets it so a retry fetches again', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({ error: 'gone' }, 404));
    fetchMock.mockResolvedValueOnce(jsonResponse(body(7)));

    await expect(loadToolCallBody(7)).rejects.toThrow('HTTP 404');
    await expect(loadToolCallBody(7)).resolves.toEqual(body(7));
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('loads several rows into one map, sharing in-flight requests', async () => {
    fetchMock.mockImplementation((input) => {
      const id = Number(String(input).split('/').pop());
      return Promise.resolve(jsonResponse(body(id)));
    });

    const [first, second] = await Promise.all([
      loadToolCallBodies([1, 2]),
      loadToolCallBodies([2, 3]),
    ]);
    expect([...first.keys()]).toEqual([1, 2]);
    expect(first.get(2)).toEqual(body(2));
    expect([...second.keys()]).toEqual([2, 3]);
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  it('goes through the get_tool_call_body command under Tauri', async () => {
    transport.isTauri = true;
    commands.getToolCallBody.mockResolvedValue(body(5));

    await expect(loadToolCallBody(5)).resolves.toEqual(body(5));
    expect(commands.getToolCallBody).toHaveBeenCalledWith(5);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
