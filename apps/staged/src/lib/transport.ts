/**
 * Transport layer abstraction for Tauri (desktop) and web (browser) modes.
 *
 * Detects the runtime environment and provides unified APIs for:
 * - Command invocation (Tauri invoke vs HTTP fetch)
 * - Event listening (Tauri events vs WebSocket)
 * - Window management (Tauri window vs no-op)
 * - Clipboard (Tauri plugin vs navigator.clipboard)
 *
 */

// ---------------------------------------------------------------------------
// Environment detection
// ---------------------------------------------------------------------------

export const isTauri: boolean = typeof window !== 'undefined' && '__TAURI__' in window;

// ---------------------------------------------------------------------------
// Command invocation
// ---------------------------------------------------------------------------

/**
 * Invoke a backend command. In Tauri mode this calls `invoke()` from the
 * Tauri API; in web mode it POSTs to `/api/invoke/{command}`.
 */
export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  if (isTauri) {
    const { invoke } = await import('@tauri-apps/api/core');
    return invoke<T>(command, args);
  }

  const response = await fetch(`/api/invoke/${encodeURIComponent(command)}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(args ?? {}),
  });

  if (!response.ok) {
    const text = await response.text();
    let message = text;
    try {
      const body = JSON.parse(text) as { error?: unknown };
      if (typeof body.error === 'string') {
        message = body.error;
      }
    } catch {
      // Non-JSON error bodies are reported as-is below.
    }
    throw new Error(message || `Command failed: ${command}`);
  }

  return (await response.json()) as T;
}

// ---------------------------------------------------------------------------
// Event listening
// ---------------------------------------------------------------------------

export type UnlistenFn = () => void;

/**
 * Listen to a backend event. In Tauri mode this delegates to the Tauri event
 * API; in web mode it connects to the shared WebSocket event stream.
 *
 * Returns a synchronous unlisten function. Registration happens asynchronously
 * in the background; if the unlisten is called before registration finishes,
 * the eventual listener is torn down on arrival. This makes the helper safe to
 * use directly in `onMount` cleanup blocks without an intermediate
 * `Promise<UnlistenFn>` reference that could race the unmount.
 */
export function listenToEvent<T>(event: string, callback: (payload: T) => void): UnlistenFn {
  if (!isTauri) {
    return webSocketListen<T>(event, callback);
  }

  let cancelled = false;
  let unlisten: UnlistenFn | undefined;

  void (async () => {
    const { listen } = await import('@tauri-apps/api/event');
    const u = await listen<T>(event, (e) => callback(e.payload));
    if (cancelled) u();
    else unlisten = u;
  })().catch((e) => {
    console.error(`[transport] Failed to register listener for event "${event}":`, e);
  });

  return () => {
    cancelled = true;
    unlisten?.();
  };
}

// ---------------------------------------------------------------------------
// WebSocket singleton for web-mode events
// ---------------------------------------------------------------------------

interface WebSocketListener {
  event: string;
  callback: (payload: unknown) => void;
}

const WEB_SOCKET_HEARTBEAT_MS = 30_000;

let ws: WebSocket | null = null;
let wsListeners: WebSocketListener[] = [];
let wsReconnectTimer: ReturnType<typeof setTimeout> | null = null;
let wsHeartbeatTimer: ReturnType<typeof setInterval> | null = null;
let wsConnecting = false;

async function getWsUrl(): Promise<string> {
  const { getPrPollClientId } = await import('./services/prPollingService');
  const url = new URL('/api/events', location.href);
  url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  url.searchParams.set('clientId', getPrPollClientId());
  return url.toString();
}

function sendHeartbeat(): void {
  if (ws?.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type: 'heartbeat' }));
  }
}

function stopHeartbeat(): void {
  if (wsHeartbeatTimer) {
    clearInterval(wsHeartbeatTimer);
    wsHeartbeatTimer = null;
  }
}

function startHeartbeat(): void {
  stopHeartbeat();
  sendHeartbeat();
  wsHeartbeatTimer = setInterval(sendHeartbeat, WEB_SOCKET_HEARTBEAT_MS);
}

function replayCurrentPrPollInterestHints(): void {
  void import('./services/prPollingService')
    .then(({ replayPrPollInterestHints }) => replayPrPollInterestHints())
    .catch((e) => {
      console.error('[transport] Failed to replay PR polling interest:', e);
    });
}

async function ensureWebSocket(): Promise<void> {
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
    return;
  }
  if (wsConnecting) return;

  wsConnecting = true;
  const url = await getWsUrl();
  if (wsListeners.length === 0) {
    wsConnecting = false;
    return;
  }

  const socket = new WebSocket(url);
  ws = socket;

  socket.onopen = () => {
    wsConnecting = false;
    startHeartbeat();
    replayCurrentPrPollInterestHints();
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
  };

  socket.onmessage = (messageEvent) => {
    try {
      const data = JSON.parse(messageEvent.data) as { event: string; payload: unknown };
      for (const listener of wsListeners) {
        if (listener.event === data.event) {
          listener.callback(data.payload);
        }
      }
    } catch {
      console.warn('[transport] Failed to parse WebSocket message:', messageEvent.data);
    }
  };

  socket.onclose = () => {
    wsConnecting = false;
    stopHeartbeat();
    if (ws === socket) {
      ws = null;
    }
    if (wsListeners.length > 0 && !wsReconnectTimer) {
      wsReconnectTimer = setTimeout(() => {
        wsReconnectTimer = null;
        if (wsListeners.length > 0) {
          void ensureWebSocket().catch((e) => {
            console.error('[transport] Failed to reconnect WebSocket:', e);
          });
        }
      }, 2000);
    }
  };

  socket.onerror = () => {
    wsConnecting = false;
  };
}

function webSocketListen<T>(event: string, callback: (payload: T) => void): UnlistenFn {
  const listener: WebSocketListener = {
    event,
    callback: callback as (payload: unknown) => void,
  };

  wsListeners.push(listener);
  void ensureWebSocket().catch((e) => {
    console.error('[transport] Failed to connect WebSocket:', e);
  });

  return () => {
    wsListeners = wsListeners.filter((l) => l !== listener);
    if (wsListeners.length === 0) {
      if (wsReconnectTimer) {
        clearTimeout(wsReconnectTimer);
        wsReconnectTimer = null;
      }
      stopHeartbeat();
      if (ws) {
        const socket = ws;
        ws = null;
        socket.close();
      }
    }
  };
}

// ---------------------------------------------------------------------------
// Window management
// ---------------------------------------------------------------------------

interface WindowHandle {
  show(): Promise<void>;
  close(): Promise<void>;
  startDragging(): Promise<void>;
  setBadgeCount(count: number | undefined): Promise<void>;
}

const noopWindow: WindowHandle = {
  show: async () => {},
  close: async () => {
    window.close();
  },
  startDragging: async () => {},
  setBadgeCount: async () => {},
};

/**
 * Get a handle to the current window. In Tauri mode this returns the real
 * Tauri window; in web mode it returns a no-op implementation.
 */
export async function getWindow(): Promise<WindowHandle> {
  if (isTauri) {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    return getCurrentWindow();
  }
  return noopWindow;
}

/**
 * Synchronous version that returns a no-op handle in web mode.
 * Useful in event handlers that can't be async.
 * In Tauri mode, dynamically imports and calls the real API.
 */
export function getWindowSync(): WindowHandle {
  if (!isTauri) return noopWindow;

  return {
    show: async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      return getCurrentWindow().show();
    },
    close: async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      return getCurrentWindow().close();
    },
    startDragging: async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      return getCurrentWindow().startDragging();
    },
    setBadgeCount: async (count: number | undefined) => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      return getCurrentWindow().setBadgeCount(count);
    },
  };
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/**
 * Write text to the clipboard. Uses Tauri's clipboard plugin in desktop mode,
 * falls back to the Web Clipboard API in browser mode.
 */
export async function writeClipboardText(text: string): Promise<void> {
  if (isTauri) {
    const { writeText } = await import('@tauri-apps/plugin-clipboard-manager');
    return writeText(text);
  }
  await navigator.clipboard.writeText(text);
}

// ---------------------------------------------------------------------------
// Drag & Drop
// ---------------------------------------------------------------------------

/**
 * Register a callback for native drag-drop events on the current webview.
 * In web mode this is a no-op (standard HTML5 drag/drop should be used instead).
 */
export async function onDragDropEvent(
  callback: (event: {
    payload: { type: string; position: { x: number; y: number }; paths: string[] };
  }) => void
): Promise<UnlistenFn> {
  if (isTauri) {
    const { getCurrentWebview } = await import('@tauri-apps/api/webview');
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    return getCurrentWebview().onDragDropEvent(callback as any);
  }
  return () => {};
}
