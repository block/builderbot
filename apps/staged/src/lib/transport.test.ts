// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  readonly url: string;
  readyState = MockWebSocket.CONNECTING;
  sent: string[] = [];
  closed = false;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;

  constructor(url: string | URL) {
    this.url = url.toString();
    sockets.push(this);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.closed = true;
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.({} as CloseEvent);
  }

  open(): void {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event('open'));
  }

  emit(data: unknown): void {
    this.onmessage?.(
      new MessageEvent('message', {
        data: typeof data === 'string' ? data : JSON.stringify(data),
      })
    );
  }
}

let sockets: MockWebSocket[];

describe('web transport', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.stubGlobal('crypto', { randomUUID: vi.fn(() => 'web-client-1') });
    sockets = [];
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it('posts commands to the web invoke endpoint', async () => {
    const fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: vi.fn().mockResolvedValue({ ok: true }),
    });
    vi.stubGlobal('fetch', fetch);

    const { invokeCommand } = await import('./transport');

    await expect(
      invokeCommand('set_focus', { clientId: 'web-client-1', focused: true })
    ).resolves.toEqual({ ok: true });

    expect(fetch).toHaveBeenCalledWith('/api/invoke/set_focus', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ clientId: 'web-client-1', focused: true }),
    });
  });

  it('connects events with the PR poll client id and sends browser heartbeats', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', MockWebSocket);

    const { listenToEvent } = await import('./transport');
    const { getPrPollClientId } = await import('./services/prPollingService');
    const callback = vi.fn();

    const unlisten = listenToEvent('pr-refresh-state', callback);
    await vi.waitFor(() => expect(sockets).toHaveLength(1));

    const socket = sockets[0];
    const url = new URL(socket.url);
    expect(url.pathname).toBe('/api/events');
    expect(url.searchParams.get('clientId')).toBe(getPrPollClientId());

    socket.open();
    expect(socket.sent).toEqual([JSON.stringify({ type: 'heartbeat' })]);

    vi.advanceTimersByTime(30_000);
    expect(socket.sent).toEqual([
      JSON.stringify({ type: 'heartbeat' }),
      JSON.stringify({ type: 'heartbeat' }),
    ]);

    socket.emit({
      event: 'pr-refresh-state',
      payload: { projectId: 'project-1', refreshing: true },
    });
    expect(callback).toHaveBeenCalledWith({ projectId: 'project-1', refreshing: true });

    unlisten();
    expect(socket.closed).toBe(true);
  });
});
