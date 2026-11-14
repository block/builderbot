import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Shortcut } from './shortcuts';

// The registry's console warning for a saved binding that lands on another
// shortcut's keys. Keyboard settings refuses such a rebind when it is made,
// so the case is a shortcut added later whose default is a key the user had
// already given to something else, as Back and Forward's ⌘[ and ⌘] can be.

let savedBindings: unknown;
let warn: ReturnType<typeof vi.spyOn>;

async function importRegistry() {
  return await import('./shortcuts');
}

function shortcut(id: string, keys: string[], description = id): Shortcut {
  return {
    id,
    description,
    category: 'app',
    keys,
    modifiers: { meta: true },
    handler: vi.fn(),
  };
}

/** What the history shortcuts register (historyShortcuts.ts). */
const back = () => shortcut('app-go-back', ['[', 'ArrowLeft'], 'Back');

beforeEach(() => {
  vi.resetModules();
  savedBindings = null;
  vi.stubGlobal('window', { addEventListener: vi.fn(), removeEventListener: vi.fn() });
  vi.stubGlobal('navigator', { platform: 'MacIntel' });
  vi.doMock('../../shared/persistentStore', () => ({
    getStoreValue: vi.fn(async () => savedBindings),
    setStoreValue: vi.fn(async () => undefined),
    deleteStoreValue: vi.fn(async () => undefined),
  }));
  warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('a saved binding on another shortcut’s keys', () => {
  it('is reported when a shortcut registers with a default the user gave to another', async () => {
    savedBindings = { 'toggle-sidebar': { keys: ['['], modifiers: { meta: true } } };
    const { initializeShortcutBindings, registerShortcut } = await importRegistry();
    await initializeShortcutBindings();
    registerShortcut(shortcut('toggle-sidebar', ['b'], 'Toggle sidebar'));
    expect(warn).not.toHaveBeenCalled();

    registerShortcut(back());

    expect(warn).toHaveBeenCalledTimes(1);
    const message = warn.mock.calls[0][0] as string;
    expect(message).toContain('⌘[');
    expect(message).toContain('"Back" (app-go-back)');
    expect(message).toContain('"Toggle sidebar" (toggle-sidebar)');
    // The one registered first is the one a press runs.
    expect(message).toContain('Only "Toggle sidebar" runs');
  });

  it('is reported when the shortcut with the saved binding registers second', async () => {
    savedBindings = { 'toggle-sidebar': { keys: ['ArrowLeft'], modifiers: { meta: true } } };
    const { initializeShortcutBindings, registerShortcut } = await importRegistry();
    await initializeShortcutBindings();
    registerShortcut(back());

    registerShortcut(shortcut('toggle-sidebar', ['b'], 'Toggle sidebar'));

    expect(warn).toHaveBeenCalledTimes(1);
    const message = warn.mock.calls[0][0] as string;
    expect(message).toContain('⌘←');
    expect(message).toContain('Only "Back" runs');
  });

  it('is reported once the saved bindings load, for shortcuts registered before', async () => {
    savedBindings = { 'toggle-sidebar': { keys: ['['], modifiers: { meta: true } } };
    const { initializeShortcutBindings, registerShortcuts } = await importRegistry();
    registerShortcuts([shortcut('toggle-sidebar', ['b'], 'Toggle sidebar'), back()]);
    expect(warn).not.toHaveBeenCalled();

    await initializeShortcutBindings();

    // Once for the pair, not once from each side.
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0][0]).toContain('Only "Toggle sidebar" runs');
  });

  it('is reported once, however often a view registers the shortcut', async () => {
    savedBindings = { 'toggle-sidebar': { keys: ['['], modifiers: { meta: true } } };
    const { initializeShortcutBindings, registerShortcut } = await importRegistry();
    await initializeShortcutBindings();
    registerShortcut(shortcut('toggle-sidebar', ['b'], 'Toggle sidebar'));

    registerShortcut(back())();
    registerShortcut(back())();
    registerShortcut(back());

    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('is not reported for the same key under other modifiers', async () => {
    savedBindings = {
      'toggle-sidebar': { keys: ['['], modifiers: { meta: true, alt: true } },
    };
    const { initializeShortcutBindings, registerShortcut } = await importRegistry();
    await initializeShortcutBindings();
    registerShortcut(shortcut('toggle-sidebar', ['b'], 'Toggle sidebar'));

    registerShortcut(back());

    expect(warn).not.toHaveBeenCalled();
  });

  it('is not reported for two defaults that share a key, saved bindings or not', async () => {
    savedBindings = { other: { keys: ['x'], modifiers: { meta: true } } };
    const { initializeShortcutBindings, registerShortcut } = await importRegistry();
    await initializeShortcutBindings();
    registerShortcut(shortcut('other', ['o']));
    registerShortcut(shortcut('view-back', ['[']));

    registerShortcut(back());

    expect(warn).not.toHaveBeenCalled();
  });

  it('changes nothing about which shortcut runs or how it is bound', async () => {
    savedBindings = { 'toggle-sidebar': { keys: ['['], modifiers: { meta: true } } };
    const { initializeShortcutBindings, registerShortcut, getShortcutBinding } =
      await importRegistry();
    await initializeShortcutBindings();
    registerShortcut(shortcut('toggle-sidebar', ['b'], 'Toggle sidebar'));
    registerShortcut(back());

    expect(getShortcutBinding('toggle-sidebar')?.keys).toEqual(['[']);
    expect(getShortcutBinding('app-go-back')?.keys).toEqual(['[', 'ArrowLeft']);
  });
});
