/**
 * App-menu listener.
 *
 * The macOS app menu is owned by the backend, which routes each item to the
 * focused window with `emit_to` (`dispatch_menu_event` in `lib.rs`). That is
 * why these must be *window-scoped* listeners: `listenToEvent` registers an
 * any-target listener, which Tauri also matches against emits addressed to
 * other windows, so one Cmd+, would open settings in every window. With no
 * window focused (every window minimized — the macOS menu stays live) nothing
 * arrives here at all: the backend drops the window-scoped items and creates a
 * New Window itself, unseeded.
 *
 * Registration and teardown are derived from the same table, so an item can't
 * be registered without also being torn down.
 *
 * The listeners are registered unconditionally. In web mode they fall through
 * to the shared WebSocket, and the web server never emits `menu:*`, so they
 * are inert.
 */

import { listenToWindowEvent, type UnlistenFn } from '../transport';
import { newWindow } from '../commands';
import { navigation, openSettings } from '../features/layout/navigation.svelte';
import { triggerShortcut } from '../features/keyboard/shortcuts';
import { runSearchShortcut } from '../features/keyboard/searchTargets';
import { increaseSize, decreaseSize, resetSize } from '../features/settings/preferences.svelte';

/**
 * Menu event → handler, mirroring the backend's menu-id → event table. Most
 * items offer a registered shortcut first refusal so the focused surface can
 * claim it (a modal's own Find, say), falling back to the app-level action.
 */
const handlers: Record<string, () => void> = {
  'menu:new-window': () => {
    // The new window inherits this window's selected project.
    void newWindow(navigation.selectedProjectId ?? null).catch((e) => {
      console.error('Failed to open new window:', e);
    });
  },
  'menu:settings': () => {
    if (!triggerShortcut('app-open-settings')) openSettings();
  },
  'menu:find': () => {
    if (!triggerShortcut('search-find')) runSearchShortcut('find');
  },
  'menu:find-next': () => {
    if (!triggerShortcut('search-find-next')) runSearchShortcut('next');
  },
  'menu:find-previous': () => {
    if (!triggerShortcut('search-find-previous')) runSearchShortcut('previous');
  },
  'menu:delete-project': () => {
    triggerShortcut('app-delete-project');
  },
  'menu:zoom-in': () => {
    if (!triggerShortcut('view-increase-size')) increaseSize();
  },
  'menu:zoom-out': () => {
    if (!triggerShortcut('view-decrease-size')) decreaseSize();
  },
  'menu:zoom-reset': () => {
    if (!triggerShortcut('view-reset-size')) resetSize();
  },
};

export function listenForMenuEvents(): UnlistenFn {
  const unlisteners = Object.entries(handlers).map(([event, handler]) =>
    listenToWindowEvent(event, handler)
  );

  return () => {
    for (const unlisten of unlisteners) unlisten();
  };
}
