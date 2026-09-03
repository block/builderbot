// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';

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
  let hydrateActiveSessions: ReturnType<typeof vi.fn>;
  let revalidateAll: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    vi.stubGlobal('crypto', { randomUUID: vi.fn(() => 'web-client-1') });
    sockets = [];
    // The busy-state hydrator pulls in rune-based stores that plain vitest
    // can't compile, so it is mocked for every socket-opening test.
    hydrateActiveSessions = vi.fn().mockResolvedValue(undefined);
    vi.doMock('./listeners/sessionStatusListener', () => ({ hydrateActiveSessions }));
    revalidateAll = vi.fn().mockResolvedValue(undefined);
    vi.doMock('./listeners/pageLifecycleListener', () => ({ revalidateAll }));
  });

  afterEach(() => {
    vi.doUnmock('./services/prPollingService');
    vi.doUnmock('./listeners/sessionStatusListener');
    vi.doUnmock('./listeners/pageLifecycleListener');
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

  it('replays PR polling interest and re-hydrates busy state when the browser event socket opens and reconnects', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', MockWebSocket);
    const replayPrPollInterestHints = vi.fn().mockResolvedValue(undefined);
    vi.doMock('./services/prPollingService', () => ({
      getPrPollClientId: () => 'web-client-1',
      replayPrPollInterestHints,
    }));

    const { listenToEvent } = await import('./transport');
    const unlisten = listenToEvent('pr-refresh-state', vi.fn());
    await vi.waitFor(() => expect(sockets).toHaveLength(1));

    sockets[0].open();
    await vi.waitFor(() => expect(replayPrPollInterestHints).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(hydrateActiveSessions).toHaveBeenCalledTimes(1));

    sockets[0].close();
    await vi.advanceTimersByTimeAsync(2000);
    await vi.waitFor(() => expect(sockets).toHaveLength(2));

    sockets[1].open();
    await vi.waitFor(() => expect(replayPrPollInterestHints).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(hydrateActiveSessions).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(revalidateAll).toHaveBeenCalledTimes(1));

    unlisten();
  });

  it('revalidates every cached surface on reconnect but not on the first connect', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', MockWebSocket);
    const { listenToEvent } = await import('./transport');
    const unlisten = listenToEvent('pr-refresh-state', vi.fn());
    await vi.waitFor(() => expect(sockets).toHaveLength(1));

    // First connect: the page's own loads are already current.
    sockets[0].open();
    await vi.waitFor(() => expect(hydrateActiveSessions).toHaveBeenCalledTimes(1));
    expect(revalidateAll).not.toHaveBeenCalled();

    // Reconnect: every event emitted during the gap is unrecoverable.
    sockets[0].close();
    await vi.advanceTimersByTimeAsync(2000);
    await vi.waitFor(() => expect(sockets).toHaveLength(2));

    sockets[1].open();
    await vi.waitFor(() => expect(revalidateAll).toHaveBeenCalledTimes(1));

    unlisten();
  });

  it('announces establishment when the event socket opens, not when the listener registers', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', MockWebSocket);

    const { listenToEvent } = await import('./transport');
    const onEstablished = vi.fn();
    const callback = vi.fn();
    const unlisten = listenToEvent('session-background-hold', callback, { onEstablished });
    await vi.waitFor(() => expect(sockets).toHaveLength(1));

    // Registration is not delivery. The socket is still connecting, the server
    // keeps no per-client queue, and an event emitted now would reach nobody —
    // so a snapshot taken here could be stale for the rest of the hold.
    expect(onEstablished).not.toHaveBeenCalled();

    sockets[0].open();
    expect(onEstablished).toHaveBeenCalledTimes(1);

    // What the hook promises: everything from here on is delivered.
    sockets[0].emit({
      event: 'session-background-hold',
      payload: { sessionId: 'session-1', holding: true },
    });
    expect(callback).toHaveBeenCalledWith({ sessionId: 'session-1', holding: true });

    unlisten();
  });

  it('announces establishment to a listener that joins an already-open socket, asynchronously', async () => {
    vi.stubGlobal('WebSocket', MockWebSocket);

    const { listenToEvent } = await import('./transport');
    const unlistenFirst = listenToEvent('pr-refresh-state', vi.fn());
    await vi.waitFor(() => expect(sockets).toHaveLength(1));
    sockets[0].open();

    const onEstablished = vi.fn();
    const unlistenSecond = listenToEvent('session-background-hold', vi.fn(), { onEstablished });

    // There is no later `onopen` to ride, so this listener is announced on its
    // own — but never synchronously, since the caller has not stored its
    // unlisten yet and a snapshot request must be cancellable.
    expect(onEstablished).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(onEstablished).toHaveBeenCalledTimes(1));

    unlistenSecond();
    unlistenFirst();
  });

  it('stays silent for a listener that unlistened before the socket opened', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', MockWebSocket);

    const { listenToEvent } = await import('./transport');
    const onEstablished = vi.fn();
    const unlisten = listenToEvent('session-background-hold', vi.fn(), { onEstablished });
    await vi.waitFor(() => expect(sockets).toHaveLength(1));

    unlisten();
    sockets[0].open();

    expect(onEstablished).not.toHaveBeenCalled();
  });

  it('announces establishment again on every reconnect', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', MockWebSocket);

    const { listenToEvent } = await import('./transport');
    const onEstablished = vi.fn();
    const unlisten = listenToEvent('session-background-hold', vi.fn(), { onEstablished });
    await vi.waitFor(() => expect(sockets).toHaveLength(1));

    sockets[0].open();
    expect(onEstablished).toHaveBeenCalledTimes(1);

    // A report emitted while the socket was down is gone for good, which is
    // the same loss as the registration gap with a different trigger — so the
    // surviving listener is announced to again rather than once per lifetime.
    sockets[0].close();
    await vi.advanceTimersByTimeAsync(2000);
    await vi.waitFor(() => expect(sockets).toHaveLength(2));

    sockets[1].open();
    expect(onEstablished).toHaveBeenCalledTimes(2);

    unlisten();
  });

  it('recovers when the server reports dropped events without reconnecting', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('WebSocket', MockWebSocket);
    const { listenToEvent } = await import('./transport');
    const callback = vi.fn();
    const unlisten = listenToEvent('project-changed', callback);
    await vi.waitFor(() => expect(sockets).toHaveLength(1));

    sockets[0].open();
    await vi.waitFor(() => expect(hydrateActiveSessions).toHaveBeenCalledTimes(1));

    sockets[0].emit({ event: 'transport:event-gap', payload: null });

    await vi.waitFor(() => expect(hydrateActiveSessions).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(revalidateAll).toHaveBeenCalledTimes(1));
    expect(callback).not.toHaveBeenCalled();
    expect(sockets).toHaveLength(1);

    unlisten();
  });
});

describe('tauri listener establishment', () => {
  let resolveListen: ((unlisten: () => void) => void) | undefined;
  let listen: ReturnType<typeof vi.fn>;
  let tauriUnlisten: ReturnType<typeof vi.fn<() => void>>;

  beforeEach(() => {
    vi.resetModules();
    vi.stubGlobal('__TAURI__', {});
    resolveListen = undefined;
    tauriUnlisten = vi.fn<() => void>();
    listen = vi.fn(
      () =>
        new Promise<() => void>((resolve) => {
          resolveListen = resolve;
        })
    );
    vi.doMock('@tauri-apps/api/event', () => ({ listen }));
  });

  afterEach(() => {
    vi.doUnmock('@tauri-apps/api/event');
    vi.unstubAllGlobals();
  });

  it('announces establishment only once the Tauri registration resolves', async () => {
    const { listenToEvent } = await import('./transport');
    const onEstablished = vi.fn();
    const unlisten = listenToEvent('session-background-hold', vi.fn(), { onEstablished });

    await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(1));
    // The gap this hook exists for: `listenToEvent` returned synchronously, but
    // registration is an awaited IPC roundtrip, and events emitted before it
    // lands are never delivered.
    expect(onEstablished).not.toHaveBeenCalled();

    resolveListen?.(tauriUnlisten);
    await vi.waitFor(() => expect(onEstablished).toHaveBeenCalledTimes(1));

    unlisten();
    expect(tauriUnlisten).toHaveBeenCalledTimes(1);
  });

  it('stays silent when the unlisten precedes registration', async () => {
    const { listenToEvent } = await import('./transport');
    const onEstablished = vi.fn();
    const unlisten = listenToEvent('session-background-hold', vi.fn(), { onEstablished });

    await vi.waitFor(() => expect(listen).toHaveBeenCalledTimes(1));
    unlisten();
    resolveListen?.(tauriUnlisten);

    // The late arrival is torn down rather than announced: there is no consumer
    // left to take a snapshot for.
    await vi.waitFor(() => expect(tauriUnlisten).toHaveBeenCalledTimes(1));
    expect(onEstablished).not.toHaveBeenCalled();
  });
});

describe('tauri window label', () => {
  let consoleError: MockInstance<typeof console.error>;

  beforeEach(() => {
    // Resets the module-level once-flag along with the module.
    vi.resetModules();
    consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.doUnmock('@tauri-apps/api/window');
    vi.unstubAllGlobals();
    consoleError.mockRestore();
  });

  it('reads the label from the injected internals', async () => {
    vi.stubGlobal('__TAURI__', {});
    vi.stubGlobal('__TAURI_INTERNALS__', { metadata: { currentWindow: { label: 'win-2' } } });

    const { getWindowLabel } = await import('./transport');

    expect(getWindowLabel()).toBe('win-2');
    expect(consoleError).not.toHaveBeenCalled();
  });

  it('reads the internals path the installed @tauri-apps/api reads', async () => {
    // The reshape tripwire. getWindowLabel() goes through the private
    // internals rather than getCurrentWindow() because the label has to be
    // available synchronously at module-init time, so an upgrade that moves
    // the label would otherwise surface only at runtime, in a user's second
    // window. getCurrentWindow() reads the same globals off the same shape,
    // so pointing the real (unmocked) package at the stub below fails here —
    // in CI, on the bump — instead. `skip: true` keeps it IPC-free.
    vi.stubGlobal('__TAURI__', {});
    vi.stubGlobal('__TAURI_INTERNALS__', { metadata: { currentWindow: { label: 'win-2' } } });

    const { getWindowLabel } = await import('./transport');
    const { getCurrentWindow } = await import('@tauri-apps/api/window');

    expect(getCurrentWindow().label).toBe('win-2');
    expect(getWindowLabel()).toBe(getCurrentWindow().label);
    expect(consoleError).not.toHaveBeenCalled();
  });

  it('reports reshaped internals once, naming the label the official API sees', async () => {
    vi.stubGlobal('__TAURI__', {});
    // The shape a Tauri upgrade might move the label to.
    vi.stubGlobal('__TAURI_INTERNALS__', { metadata: { window: { label: 'win-2' } } });
    vi.doMock('@tauri-apps/api/window', () => ({
      getCurrentWindow: () => ({ label: 'win-2' }),
    }));

    const { getWindowLabel } = await import('./transport');

    expect(getWindowLabel()).toBeNull();
    expect(consoleError).toHaveBeenCalledTimes(1);
    expect(consoleError.mock.calls[0][0]).toContain('Tauri window label unavailable');

    await vi.waitFor(() => expect(consoleError).toHaveBeenCalledTimes(2));
    expect(consoleError.mock.calls[1][0]).toContain('the current window label is "win-2"');

    // Every navigation persist calls through here, so a second failed lookup
    // must stay quiet.
    expect(getWindowLabel()).toBeNull();
    expect(consoleError).toHaveBeenCalledTimes(2);
  });

  it('stays silent in web mode, where a null label is the expected answer', async () => {
    const { getWindowLabel } = await import('./transport');

    expect(getWindowLabel()).toBeNull();
    expect(consoleError).not.toHaveBeenCalled();
  });
});
