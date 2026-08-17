/**
 * Listener for the backend's `app:quit-requested` event.
 *
 * `Cmd+Q` / the app-menu Quit item reach `app_lifecycle::request_quit`, which
 * emits this event instead of exiting when sessions are still active. The
 * backend addresses it to exactly one window with `emit_to`, so this must be a
 * *window-scoped* listener — the any-target `listenToEvent` also matches emits
 * addressed to other windows, and every window would raise its own dialog.
 * Wired at App level so it works on any route, and Tauri-only: quitting is a
 * desktop-host action, and the `confirm_quit` command a browser client would
 * need is deliberately absent from the web-mode dispatch table.
 */

import { isTauri, listenToWindowEvent, type UnlistenFn } from '../transport';
import { quitPrompt } from '../stores/quitPrompt.svelte';
import type { QuitRequestedPayload } from '../types';

export function listenForQuitRequests(): UnlistenFn {
  if (!isTauri) return () => {};

  return listenToWindowEvent<QuitRequestedPayload>('app:quit-requested', (payload) => {
    quitPrompt.requested(payload);
  });
}
