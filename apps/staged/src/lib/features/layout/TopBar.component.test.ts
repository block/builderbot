import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The top bar as a whole, in the desktop app's window on a Mac: where a
// press drags the window, and the room kept for the traffic lights. The
// history buttons' own behaviour is in TopBarNavControls.component.test.ts.

const spies = vi.hoisted(() => ({
  startDragging: vi.fn(),
  goBack: vi.fn(async () => true),
  goForward: vi.fn(async () => true),
}));

vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  isTauri: true,
  getWindowSync: () => ({ startDragging: spies.startDragging }),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isFullscreen: async () => false,
    onResized: async () => () => {},
  }),
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
import TopBar from './TopBar.svelte';
import { world } from './testStubs/historyWorld.svelte';

let component: ReturnType<typeof mount> | null = null;

const bar = () => document.querySelector<HTMLElement>('.top-bar')!;
const spacer = () => document.querySelector<HTMLElement>('.traffic-light-spacer')!;
const navGroup = () => document.querySelector<HTMLElement>('.top-bar-nav')!;
const button = (label: string) =>
  document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;

/** A primary-button press on `target`, as the bar's drag handler sees it. */
function press(target: Element, button = 0) {
  target.dispatchEvent(new MouseEvent('pointerdown', { button, bubbles: true, cancelable: true }));
}

function mountBar() {
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(TopBar, { target });
  flushSync();
}

beforeEach(() => {
  world.reset();
  Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true });
  viewport.isMobile = false;
  viewport.hasCoarsePointer = false;
  for (const spy of Object.values(spies)) spy.mockClear();
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('TopBar window drag', () => {
  beforeEach(mountBar);

  it('drags the window from an empty part of the bar', () => {
    press(bar());
    press(document.querySelector('.drag-spacer')!);

    expect(spies.startDragging).toHaveBeenCalledTimes(2);
  });

  it('does not drag from a press on a disabled history button', () => {
    // No history: Back and Forward are both disabled. A disabled button
    // takes no pointer events, so the press lands on its wrapper.
    expect(button('Back').disabled).toBe(true);
    expect(button('Forward').disabled).toBe(true);

    press(button('Back').parentElement!);
    press(button('Forward').parentElement!);

    expect(spies.startDragging).not.toHaveBeenCalled();
    expect(spies.goBack).not.toHaveBeenCalled();
    expect(spies.goForward).not.toHaveBeenCalled();
  });

  it('does not drag from anywhere in the history and search group', () => {
    world.canGoBack = true;
    flushSync();

    // The gap between two buttons, an enabled button, and an icon in one.
    press(navGroup());
    press(button('Back'));
    press(button('Search projects').querySelector('svg')!);

    expect(spies.startDragging).not.toHaveBeenCalled();
  });

  it('still drags from beside the group', () => {
    // The group's parent holds the route's own left actions too; around
    // them it is bar like any other.
    press(navGroup().parentElement!);

    expect(spies.startDragging).toHaveBeenCalledTimes(1);
  });

  it('drags only on the primary button', () => {
    press(bar(), 2);

    expect(spies.startDragging).not.toHaveBeenCalled();
  });
});

describe('TopBar traffic-light room', () => {
  const room = () => bar().style.getPropertyValue('--traffic-light-spacer-width');

  it('is kept ahead of the history buttons in a desktop-width window', () => {
    mountBar();

    expect(room()).toBe('70px');
    expect(spacer().classList.contains('collapsed')).toBe(false);
    // The spacer comes first, so the buttons start after the window controls.
    expect(bar().firstElementChild).toBe(spacer());
    expect(spacer().nextElementSibling!.contains(navGroup())).toBe(true);
  });

  it('is kept in a window narrower than the mobile breakpoint', () => {
    // A narrow Mac window takes the mobile layout but keeps a fine pointer
    // and its traffic lights. The mobile CSS drops only a collapsed spacer.
    viewport.isMobile = true;
    mountBar();

    expect(room()).toBe('70px');
    expect(spacer().classList.contains('collapsed')).toBe(false);
  });

  it('follows the window as it is narrowed and widened', () => {
    mountBar();

    viewport.isMobile = true;
    flushSync();
    expect(room()).toBe('70px');

    viewport.isMobile = false;
    flushSync();
    expect(room()).toBe('70px');
  });

  it('is dropped on a touch device', () => {
    viewport.isMobile = true;
    viewport.hasCoarsePointer = true;
    mountBar();

    expect(room()).toBe('0px');
    expect(spacer().classList.contains('collapsed')).toBe(true);
  });
});
