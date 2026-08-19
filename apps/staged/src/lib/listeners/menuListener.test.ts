import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The app-menu listener. The properties worth pinning are structural: exactly
 * the nine backend menu events are registered, they're window-scoped (the
 * backend emits to the focused window, so an any-target listener would fire in
 * every window), and every one of them is torn down — the omission this module
 * exists to make impossible.
 */

type EventCallback = (payload: unknown) => void;

/** Mirrors `MENU_ITEMS` in `lib.rs`, hard-coded rather than read from the module. */
const MENU_EVENTS = [
  'menu:new-window',
  'menu:settings',
  'menu:find',
  'menu:find-next',
  'menu:find-previous',
  'menu:delete-project',
  'menu:zoom-in',
  'menu:zoom-out',
  'menu:zoom-reset',
];

let windowCallbacks: Map<string, EventCallback>;
let unlistenSpies: Map<string, ReturnType<typeof vi.fn>>;
let listenToEvent: ReturnType<typeof vi.fn>;
let newWindow: ReturnType<typeof vi.fn>;
let triggerShortcut: ReturnType<typeof vi.fn>;
let runSearchShortcut: ReturnType<typeof vi.fn>;
let openSettings: ReturnType<typeof vi.fn>;
let increaseSize: ReturnType<typeof vi.fn>;
let decreaseSize: ReturnType<typeof vi.fn>;
let resetSize: ReturnType<typeof vi.fn>;
let navigation: { selectedProjectId: string | null };

async function startListening() {
  const { listenForMenuEvents } = await import('./menuListener');
  return listenForMenuEvents();
}

function fire(event: string): void {
  const callback = windowCallbacks.get(event);
  if (!callback) throw new Error(`no listener registered for ${event}`);
  callback(undefined);
}

beforeEach(() => {
  vi.resetModules();
  windowCallbacks = new Map();
  unlistenSpies = new Map();
  listenToEvent = vi.fn();
  newWindow = vi.fn(() => Promise.resolve('win-2'));
  triggerShortcut = vi.fn(() => false);
  runSearchShortcut = vi.fn(() => true);
  openSettings = vi.fn();
  increaseSize = vi.fn();
  decreaseSize = vi.fn();
  resetSize = vi.fn();
  navigation = { selectedProjectId: null };

  vi.doMock('../transport', () => ({
    listenToEvent,
    listenToWindowEvent: (event: string, callback: EventCallback) => {
      windowCallbacks.set(event, callback);
      const unlisten = vi.fn(() => windowCallbacks.delete(event));
      unlistenSpies.set(event, unlisten);
      return unlisten;
    },
  }));
  vi.doMock('../commands', () => ({ newWindow }));
  vi.doMock('../features/layout/navigation.svelte', () => ({
    get navigation() {
      return navigation;
    },
    openSettings,
  }));
  vi.doMock('../features/keyboard/shortcuts', () => ({ triggerShortcut }));
  vi.doMock('../features/keyboard/searchTargets', () => ({ runSearchShortcut }));
  vi.doMock('../features/settings/preferences.svelte', () => ({
    increaseSize,
    decreaseSize,
    resetSize,
  }));
});

afterEach(() => {
  vi.doUnmock('../transport');
  vi.doUnmock('../commands');
  vi.doUnmock('../features/layout/navigation.svelte');
  vi.doUnmock('../features/keyboard/shortcuts');
  vi.doUnmock('../features/keyboard/searchTargets');
  vi.doUnmock('../features/settings/preferences.svelte');
});

describe('listenForMenuEvents', () => {
  it('registers exactly the nine menu events, window-scoped', async () => {
    await startListening();

    expect([...windowCallbacks.keys()]).toEqual(MENU_EVENTS);
    // Any-target listeners would also match emits addressed to other windows.
    expect(listenToEvent).not.toHaveBeenCalled();
  });

  it('tears down every listener it registered', async () => {
    const unlistenMenu = await startListening();

    unlistenMenu();

    expect(unlistenSpies.size).toBe(MENU_EVENTS.length);
    for (const [event, unlisten] of unlistenSpies) {
      expect(unlisten, `${event} should be unlistened exactly once`).toHaveBeenCalledTimes(1);
    }
    expect(windowCallbacks.size).toBe(0);
  });

  it('opens a new window seeded with the opener’s selected project', async () => {
    navigation.selectedProjectId = 'p1';
    await startListening();

    fire('menu:new-window');

    expect(newWindow).toHaveBeenCalledWith('p1');
  });

  it('passes a null seed when no project is selected, and swallows failures', async () => {
    newWindow.mockImplementation(() => Promise.reject(new Error('build failed')));
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    await startListening();

    fire('menu:new-window');
    await Promise.resolve();

    expect(newWindow).toHaveBeenCalledWith(null);
    // Caught, so it can't escape as an unhandled rejection.
    expect(consoleError).toHaveBeenCalledTimes(1);
    consoleError.mockRestore();
  });

  it('gives a registered shortcut first refusal before the app-level fallback', async () => {
    await startListening();

    const cases: {
      event: string;
      shortcut: string;
      fallback: () => ReturnType<typeof vi.fn>;
      arg?: string;
    }[] = [
      { event: 'menu:settings', shortcut: 'app-open-settings', fallback: () => openSettings },
      {
        event: 'menu:find',
        shortcut: 'search-find',
        fallback: () => runSearchShortcut,
        arg: 'find',
      },
      {
        event: 'menu:find-next',
        shortcut: 'search-find-next',
        fallback: () => runSearchShortcut,
        arg: 'next',
      },
      {
        event: 'menu:find-previous',
        shortcut: 'search-find-previous',
        fallback: () => runSearchShortcut,
        arg: 'previous',
      },
      { event: 'menu:zoom-in', shortcut: 'view-increase-size', fallback: () => increaseSize },
      { event: 'menu:zoom-out', shortcut: 'view-decrease-size', fallback: () => decreaseSize },
      { event: 'menu:zoom-reset', shortcut: 'view-reset-size', fallback: () => resetSize },
    ];
    const allFallbacks = [openSettings, runSearchShortcut, increaseSize, decreaseSize, resetSize];

    for (const { event, shortcut, fallback, arg } of cases) {
      // Claimed by the focused surface → no fallback runs.
      vi.clearAllMocks();
      triggerShortcut.mockReturnValue(true);
      fire(event);
      expect(triggerShortcut, event).toHaveBeenCalledExactlyOnceWith(shortcut);
      for (const spy of allFallbacks) expect(spy, event).not.toHaveBeenCalled();

      // Unclaimed → the app-level action runs.
      vi.clearAllMocks();
      triggerShortcut.mockReturnValue(false);
      fire(event);
      const expected = arg === undefined ? [] : [arg];
      expect(fallback(), event).toHaveBeenCalledExactlyOnceWith(...expected);
    }
  });

  it('routes delete-project to its shortcut with no fallback', async () => {
    await startListening();

    fire('menu:delete-project');

    expect(triggerShortcut).toHaveBeenCalledExactlyOnceWith('app-delete-project');
    expect(openSettings).not.toHaveBeenCalled();
    expect(runSearchShortcut).not.toHaveBeenCalled();
  });
});
