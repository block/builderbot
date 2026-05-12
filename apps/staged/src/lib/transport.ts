/**
 * Transport layer abstraction for Tauri (desktop) and web (browser) modes.
 *
 * Detects the runtime environment and provides unified APIs for:
 * - Command invocation (Tauri invoke vs HTTP fetch)
 * - Event listening (Tauri events vs WebSocket)
 * - Window management (Tauri window vs no-op)
 * - Clipboard (Tauri plugin vs navigator.clipboard)
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

  const response = await fetch(`/api/invoke/${command}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(args ?? {}),
  });

  if (response.status === 401) {
    redirectToLogin();
    throw new Error('Authentication required');
  }

  if (!response.ok) {
    // The Axum server returns `{ "error": "..." }` JSON for BAD_REQUEST responses.
    // Parse the JSON and extract the error field for a clean error message.
    const text = await response.text();
    try {
      const body = JSON.parse(text);
      if (body?.error) {
        throw new Error(body.error);
      }
    } catch (e) {
      if (e instanceof Error && e.message !== text) throw e;
    }
    throw new Error(text);
  }

  return response.json();
}

// ---------------------------------------------------------------------------
// Web authentication
// ---------------------------------------------------------------------------

let loginRedirectPending = false;

function redirectToLogin(): void {
  if (loginRedirectPending) return;
  loginRedirectPending = true;
  // Use a small delay to batch multiple 401s that fire simultaneously
  setTimeout(() => {
    window.location.hash = '#/login';
    loginRedirectPending = false;
  }, 50);
}

/**
 * Submit a bearer token to the web server's auth endpoint.
 * On success the server sets a session cookie and subsequent requests are authenticated.
 */
export async function submitWebToken(token: string): Promise<boolean> {
  const response = await fetch('/api/auth', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token }),
  });
  return response.ok;
}

// ---------------------------------------------------------------------------
// Event listening
// ---------------------------------------------------------------------------

export type UnlistenFn = () => void;

/**
 * Listen to a backend event. In Tauri mode this delegates to the Tauri event
 * API; in web mode it connects to a shared WebSocket and filters by event name.
 */
export async function listenToEvent<T>(
  event: string,
  callback: (payload: T) => void
): Promise<UnlistenFn> {
  if (isTauri) {
    const { listen } = await import('@tauri-apps/api/event');
    return listen<T>(event, (e) => callback(e.payload));
  }

  return webSocketListen<T>(event, callback);
}

// ---------------------------------------------------------------------------
// WebSocket singleton for web-mode events
// ---------------------------------------------------------------------------

interface WebSocketListener {
  event: string;
  callback: (payload: unknown) => void;
}

let ws: WebSocket | null = null;
let wsListeners: WebSocketListener[] = [];
let wsReconnectTimer: ReturnType<typeof setTimeout> | null = null;
let wsConnecting = false;

function getWsUrl(): string {
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${protocol}//${location.host}/api/events`;
}

function ensureWebSocket(): void {
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
    return;
  }
  if (wsConnecting) return;

  wsConnecting = true;
  ws = new WebSocket(getWsUrl());

  ws.onopen = () => {
    wsConnecting = false;
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
  };

  ws.onmessage = (messageEvent) => {
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

  ws.onclose = () => {
    wsConnecting = false;
    // Auto-reconnect if there are still listeners
    if (wsListeners.length > 0 && !wsReconnectTimer) {
      wsReconnectTimer = setTimeout(() => {
        wsReconnectTimer = null;
        if (wsListeners.length > 0) {
          ensureWebSocket();
        }
      }, 2000);
    }
  };

  ws.onerror = () => {
    wsConnecting = false;
    // onclose will fire after onerror, which handles reconnect
  };
}

function webSocketListen<T>(event: string, callback: (payload: T) => void): UnlistenFn {
  const listener: WebSocketListener = {
    event,
    callback: callback as (payload: unknown) => void,
  };

  wsListeners.push(listener);
  ensureWebSocket();

  return () => {
    wsListeners = wsListeners.filter((l) => l !== listener);
    // Tear down WebSocket when no listeners remain
    if (wsListeners.length === 0) {
      if (wsReconnectTimer) {
        clearTimeout(wsReconnectTimer);
        wsReconnectTimer = null;
      }
      if (ws) {
        ws.close();
        ws = null;
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
    // In browser mode, just close the tab/window
    window.close();
  },
  startDragging: async () => {},
  setBadgeCount: async () => {},
};

/**
 * Get a handle to the current window. In Tauri mode this returns the real
 * Tauri window; in web mode it returns a no-op (or limited) implementation.
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

  // Return a proxy that lazily imports the Tauri window API
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
  // No-op in web mode — native file drag is a Tauri-only feature
  return () => {};
}
