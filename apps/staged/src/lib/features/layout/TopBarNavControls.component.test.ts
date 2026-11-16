import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The top bar's Back, Forward and Search buttons, with the history they read
// replaced by rune state the test owns (testStubs/historyWorld) and the real
// shortcut registry and palette state behind Search and the tooltips. What is
// covered here is the wiring: which button is enabled, what a press does,
// and what each is called. The history itself is covered in
// navigationHistory.test.ts and navigation.test.ts.

const spies = vi.hoisted(() => ({
  goBack: vi.fn(async () => true),
  goForward: vi.fn(async () => true),
}));

vi.mock('./navigation.svelte', async () => {
  const { world } = await import('./testStubs/historyWorld.svelte');
  return {
    navigation: {
      get canGoBack() {
        return world.canGoBack;
      },
      get canGoForward() {
        return world.canGoForward;
      },
    },
    goBack: spies.goBack,
    goForward: spies.goForward,
  };
});

import { viewport } from '../../shared/viewport.svelte';
import {
  closeCommandPalette,
  commandPalette,
  commandPaletteShortcut,
} from '../commandPalette/paletteState.svelte';
import {
  registerShortcuts,
  resetShortcutBinding,
  updateShortcutBinding,
} from '../keyboard/shortcuts';
import { GO_BACK_SHORTCUT_ID, historyShortcuts } from './historyShortcuts';
import TopBarNavControls from './TopBarNavControls.svelte';
import { world } from './testStubs/historyWorld.svelte';

let component: ReturnType<typeof mount> | null = null;

const group = () => document.querySelector<HTMLElement>('[role="group"]')!;
const buttons = () => [...group().querySelectorAll<HTMLButtonElement>('button')];
const button = (label: string) => buttons().find((el) => el.getAttribute('aria-label') === label)!;
/** The tooltip, which is on the button's wrapper (a disabled button takes
 *  no pointer events, so one of its own would never show). */
const tooltip = (label: string) => button(label).parentElement!.title;

/** The registry reads the platform from the navigator: Command on a Mac,
 *  Ctrl elsewhere. */
function setPlatform(platform: string) {
  Object.defineProperty(navigator, 'platform', { value: platform, configurable: true });
}

beforeEach(() => {
  world.reset();
  viewport.showShortcutHints = true;
  viewport.isMobile = false;
  setPlatform('MacIntel');
  for (const spy of Object.values(spies)) spy.mockClear();
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(TopBarNavControls, { target });
  flushSync();
});

afterEach(() => {
  closeCommandPalette();
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('TopBarNavControls', () => {
  it('is Back, Forward and Search, in that order, as one labelled group', () => {
    expect(group().getAttribute('aria-label')).toBe('History and search');
    expect(buttons().map((el) => el.getAttribute('aria-label'))).toEqual([
      'Back',
      'Forward',
      'Search projects',
    ]);
    // Buttons, so a press on one never starts a window drag (TopBar's
    // startDrag skips them).
    expect(buttons().every((el) => el.closest('button, a, input, [role="button"]'))).toBe(true);
  });

  it('names each button alone while its shortcut is not registered', () => {
    // App.svelte registers them once the saved bindings have loaded.
    expect(tooltip('Back')).toBe('Back');
    expect(tooltip('Forward')).toBe('Forward');
    expect(tooltip('Search projects')).toBe('Search projects');
  });
});

// The tooltips name the keys each shortcut is bound to in the registry, not
// keys written into the component.
describe('TopBarNavControls tooltips', () => {
  let unregister: () => void;

  beforeEach(() => {
    // The definitions App.svelte registers, registered after the buttons
    // mounted, as there.
    unregister = registerShortcuts([...historyShortcuts(), commandPaletteShortcut()]);
    flushSync();
  });

  afterEach(async () => {
    await resetShortcutBinding(GO_BACK_SHORTCUT_ID);
    unregister();
  });

  it('names each button with the keys its shortcut is bound to', () => {
    expect(tooltip('Back')).toBe('Back (⌘[)');
    expect(tooltip('Forward')).toBe('Forward (⌘])');
    expect(tooltip('Search projects')).toBe('Search projects (⌘K)');
  });

  it('names Ctrl where the registry maps the Command modifier to it', () => {
    unregister();
    setPlatform('Win32');
    unregister = registerShortcuts([...historyShortcuts(), commandPaletteShortcut()]);
    flushSync();

    expect(tooltip('Back')).toBe('Back (Ctrl+[)');
    expect(tooltip('Forward')).toBe('Forward (Ctrl+])');
    expect(tooltip('Search projects')).toBe('Search projects (Ctrl+K)');
  });

  it('follows a rebind in Keyboard settings, and its reset', async () => {
    await updateShortcutBinding(GO_BACK_SHORTCUT_ID, ['b'], { meta: true, shift: true });
    flushSync();

    expect(tooltip('Back')).toBe('Back (⇧⌘B)');
    // The others are as they were.
    expect(tooltip('Forward')).toBe('Forward (⌘])');

    await resetShortcutBinding(GO_BACK_SHORTCUT_ID);
    flushSync();

    expect(tooltip('Back')).toBe('Back (⌘[)');
  });

  it('drops the keys when the shortcut is withdrawn', () => {
    unregister();
    flushSync();

    expect(tooltip('Back')).toBe('Back');
    expect(tooltip('Search projects')).toBe('Search projects');
    unregister = () => {};
  });

  it('leaves the shortcuts out where there is no keyboard to press them on', () => {
    viewport.showShortcutHints = false;
    flushSync();

    expect(tooltip('Back')).toBe('Back');
    expect(tooltip('Forward')).toBe('Forward');
    expect(tooltip('Search projects')).toBe('Search projects');
  });
});

describe('TopBarNavControls history buttons', () => {
  it('disables both where the history has one entry', () => {
    expect(button('Back').disabled).toBe(true);
    expect(button('Forward').disabled).toBe(true);
  });

  it('enables Back alone once there is an earlier entry', () => {
    world.canGoBack = true;
    flushSync();

    expect(button('Back').disabled).toBe(false);
    expect(button('Forward').disabled).toBe(true);
  });

  it('enables Forward alone at the first entry of a longer history', () => {
    world.canGoForward = true;
    flushSync();

    expect(button('Back').disabled).toBe(true);
    expect(button('Forward').disabled).toBe(false);
  });

  it('follows the history as it changes: Forward goes when a new view is visited', () => {
    world.canGoBack = true;
    world.canGoForward = true;
    flushSync();
    expect(button('Back').disabled).toBe(false);
    expect(button('Forward').disabled).toBe(false);

    world.canGoForward = false;
    flushSync();

    expect(button('Back').disabled).toBe(false);
    expect(button('Forward').disabled).toBe(true);
  });

  it('steps back on a press of Back, and only back', () => {
    world.canGoBack = true;
    world.canGoForward = true;
    flushSync();

    button('Back').click();

    expect(spies.goBack).toHaveBeenCalledTimes(1);
    expect(spies.goForward).not.toHaveBeenCalled();
  });

  it('steps forward on a press of Forward, and only forward', () => {
    world.canGoBack = true;
    world.canGoForward = true;
    flushSync();

    button('Forward').click();

    expect(spies.goForward).toHaveBeenCalledTimes(1);
    expect(spies.goBack).not.toHaveBeenCalled();
  });

  it('does nothing on a press of a disabled button', () => {
    button('Back').click();
    button('Forward').click();

    expect(spies.goBack).not.toHaveBeenCalled();
    expect(spies.goForward).not.toHaveBeenCalled();
  });
});

// The mobile layout shows Forward last and hides it while there is nothing
// forward, both in the component's styles, which jsdom does not apply. What
// is covered here is what the script does about it: the markup's order, and
// the focus of a Forward that is about to be hidden.
describe('TopBarNavControls in the mobile layout', () => {
  /** There is an entry each way, or only the ones named. */
  function show({ back = true, forward = true } = {}) {
    world.canGoBack = back;
    world.canGoForward = forward;
    flushSync();
  }

  /** The last step forward is taken: nothing is forward any more. */
  async function takeLastStepForward({ back = true } = {}) {
    world.canGoBack = back;
    world.canGoForward = false;
    flushSync();
    await tick();
  }

  beforeEach(() => {
    viewport.isMobile = true;
    flushSync();
  });

  it('keeps Back, Forward, Search as the order they are tabbed and read in', () => {
    show();

    expect(buttons().map((el) => el.getAttribute('aria-label'))).toEqual([
      'Back',
      'Forward',
      'Search projects',
    ]);
  });

  it('marks Forward for the styles that place it last and hide it', () => {
    const wrapper = () => button('Forward').parentElement!;
    expect(wrapper().classList.contains('forward')).toBe(true);
    expect(wrapper().classList.contains('nowhere-forward')).toBe(true);

    show();

    expect(wrapper().classList.contains('forward')).toBe(true);
    expect(wrapper().classList.contains('nowhere-forward')).toBe(false);
  });

  it('moves the focus from a Forward that is hidden to Back', async () => {
    show();
    button('Forward').focus();
    expect(document.activeElement).toBe(button('Forward'));

    await takeLastStepForward();

    expect(document.activeElement).toBe(button('Back'));
  });

  it('moves it to Back even where Back is only then enabled', async () => {
    // At the first entry, Forward focused: the step forward enables Back in
    // the same update that hides Forward.
    show({ back: false });
    button('Forward').focus();

    await takeLastStepForward();

    expect(document.activeElement).toBe(button('Back'));
  });

  it('moves it to Search where there is nothing back either', async () => {
    // The history was cut down to the one entry on screen.
    show();
    button('Forward').focus();

    await takeLastStepForward({ back: false });

    expect(document.activeElement).toBe(button('Search projects'));
  });

  it('leaves the focus alone where Forward did not have it', async () => {
    show();
    button('Search projects').focus();

    await takeLastStepForward();

    expect(document.activeElement).toBe(button('Search projects'));
  });

  it('leaves the focus where something else took it meanwhile', async () => {
    const field = document.createElement('input');
    document.body.appendChild(field);
    show();
    button('Forward').focus();

    world.canGoForward = false;
    flushSync();
    field.focus();
    await tick();

    expect(document.activeElement).toBe(field);
  });

  it('does not move the focus in the desktop layout, where Forward stays in place', async () => {
    viewport.isMobile = false;
    show();
    button('Forward').focus();

    await takeLastStepForward();

    expect(document.activeElement).not.toBe(button('Back'));
    expect(document.activeElement).not.toBe(button('Search projects'));
  });
});

describe('TopBarNavControls Search', () => {
  let unregister: () => void;
  let enabled: boolean;

  beforeEach(() => {
    enabled = true;
    // The definition App.svelte registers, through the registry it uses.
    unregister = registerShortcuts([commandPaletteShortcut(() => enabled)]);
  });

  afterEach(() => unregister());

  it('is always enabled, and says it opens a dialog', () => {
    expect(button('Search projects').disabled).toBe(false);
    expect(button('Search projects').getAttribute('aria-haspopup')).toBe('dialog');
  });

  it('opens the palette, as ⌘K does', () => {
    button('Search projects').click();

    expect(commandPalette.open).toBe(true);
    expect(spies.goBack).not.toHaveBeenCalled();
    expect(spies.goForward).not.toHaveBeenCalled();
  });

  it('runs the handler registered for ⌘K rather than one of its own', () => {
    unregister();
    const handler = vi.fn();
    unregister = registerShortcuts([{ ...commandPaletteShortcut(), handler }]);

    button('Search projects').click();

    expect(handler).toHaveBeenCalledTimes(1);
    expect(commandPalette.open).toBe(false);
  });

  it('toggles like the key: a second press closes the palette', () => {
    button('Search projects').click();
    button('Search projects').click();

    expect(commandPalette.open).toBe(false);
  });

  it('does not open over another dialog, as ⌘K does not', () => {
    const other = document.createElement('div');
    other.setAttribute('role', 'dialog');
    document.body.appendChild(other);

    button('Search projects').click();

    expect(commandPalette.open).toBe(false);
  });

  it('does nothing where there is no workspace to search, as ⌘K does not', () => {
    enabled = false;

    button('Search projects').click();

    expect(commandPalette.open).toBe(false);
  });

  it('does nothing where the shortcut is not registered', () => {
    unregister();

    button('Search projects').click();

    expect(commandPalette.open).toBe(false);
  });
});
