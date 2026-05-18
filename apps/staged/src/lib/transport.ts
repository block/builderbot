/**
 * Transport layer abstraction for Tauri (desktop) and web (browser) modes.
 *
 * Detects the runtime environment and provides unified APIs for:
 * - Command invocation (Tauri invoke vs HTTP fetch)
 * - Event listening (Tauri events vs WebSocket)
 * - Window management (Tauri window vs no-op)
 * - Clipboard (Tauri plugin vs navigator.clipboard)
 *
 * NOTE: Web-mode implementations are intentionally stubbed out in this build.
 * TODO(web): restore web transport paths from the `mobile-web` branch.
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
 * Tauri API; in web mode it is stubbed out.
 */
export async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  if (isTauri) {
    const { invoke } = await import('@tauri-apps/api/core');
    return invoke<T>(command, args);
  }

  // TODO(web): restore HTTP transport from the `mobile-web` branch
  throw new Error(`[transport] Web mode is not available in this build (command: ${command})`);
}

// ---------------------------------------------------------------------------
// Event listening
// ---------------------------------------------------------------------------

export type UnlistenFn = () => void;

/**
 * Listen to a backend event. In Tauri mode this delegates to the Tauri event
 * API; in web mode it is stubbed out.
 *
 * Returns a synchronous unlisten function. Registration happens asynchronously
 * in the background; if the unlisten is called before registration finishes,
 * the eventual listener is torn down on arrival. This makes the helper safe to
 * use directly in `onMount` cleanup blocks without an intermediate
 * `Promise<UnlistenFn>` reference that could race the unmount.
 */
export function listenToEvent<T>(event: string, callback: (payload: T) => void): UnlistenFn {
  let cancelled = false;
  let unlisten: UnlistenFn | undefined;

  void (async () => {
    if (!isTauri) {
      // TODO(web): restore WebSocket event transport from the `mobile-web` branch
      console.warn(`[transport] Web mode event listening stubbed out (event: ${event})`);
      return;
    }
    const { listen } = await import('@tauri-apps/api/event');
    const u = await listen<T>(event, (e) => callback(e.payload));
    if (cancelled) u();
    else unlisten = u;
  })();

  return () => {
    cancelled = true;
    unlisten?.();
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
