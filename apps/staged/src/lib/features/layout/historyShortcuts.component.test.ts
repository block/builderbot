import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The Back and Forward shortcuts as App.svelte registers them: the
// definitions from historyShortcuts through the real registry and its
// window-level key handler, with the two navigation functions replaced by
// spies. What the functions do with a step is covered in navigation.test.ts.

const spies = vi.hoisted(() => ({
  goBack: vi.fn(async () => true),
  goForward: vi.fn(async () => true),
}));

vi.mock('./navigation.svelte', () => ({
  goBack: spies.goBack,
  goForward: spies.goForward,
}));

import appSource from '../../../App.svelte?raw';
import {
  getAllShortcuts,
  hasShortcutConflict,
  registerShortcuts,
  resetShortcutBinding,
  updateShortcutBinding,
} from '../keyboard/shortcuts';
import { GO_BACK_SHORTCUT_ID, GO_FORWARD_SHORTCUT_ID, historyShortcuts } from './historyShortcuts';

let unregister: () => void;

/** Press a key with Command held (the registry's `meta` on a Mac), on
 *  `target`. Returns the event, to see whether the key was swallowed. */
function press(key: string, target: EventTarget = document.body, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent('keydown', {
    key,
    metaKey: true,
    bubbles: true,
    cancelable: true,
    ...init,
  });
  target.dispatchEvent(event);
  return event;
}

beforeEach(() => {
  Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true });
  for (const spy of Object.values(spies)) spy.mockClear();
  unregister = registerShortcuts(historyShortcuts());
});

afterEach(async () => {
  await resetShortcutBinding(GO_BACK_SHORTCUT_ID);
  unregister();
  document.body.innerHTML = '';
});

describe('the Back and Forward shortcuts', () => {
  it('are registered by the app with its other shortcuts', () => {
    // App.svelte cannot be mounted here, so this reads its source: the
    // definitions tested below are the ones it hands to the registry.
    const registration = appSource.slice(appSource.indexOf('registerShortcuts(['));
    expect(registration).toContain('...historyShortcuts(),');
    expect(appSource).toContain("from './lib/features/layout/historyShortcuts'");
  });

  it('are listed for Keyboard settings as Back and Forward', () => {
    const listed = getAllShortcuts().filter((shortcut) =>
      [GO_BACK_SHORTCUT_ID, GO_FORWARD_SHORTCUT_ID].includes(shortcut.id)
    );

    expect(
      listed.map(({ id, description, category, keys, modifiers }) => [
        id,
        description,
        category,
        keys,
        modifiers?.meta,
      ])
    ).toEqual([
      ['app-go-back', 'Back', 'app', ['[', 'ArrowLeft'], true],
      ['app-go-forward', 'Forward', 'app', [']', 'ArrowRight'], true],
    ]);
    // Their keys are theirs alone: a rebind of anything else onto one is
    // refused in Keyboard settings.
    expect(hasShortcutConflict(['['], { meta: true })).toBe(GO_BACK_SHORTCUT_ID);
    expect(hasShortcutConflict(['ArrowRight'], { meta: true })).toBe(GO_FORWARD_SHORTCUT_ID);
  });

  it('step Back on ⌘[ and ⌘←', () => {
    expect(press('[').defaultPrevented).toBe(true);
    expect(press('ArrowLeft').defaultPrevented).toBe(true);

    expect(spies.goBack).toHaveBeenCalledTimes(2);
    expect(spies.goForward).not.toHaveBeenCalled();
  });

  it('step Forward on ⌘] and ⌘→', () => {
    expect(press(']').defaultPrevented).toBe(true);
    expect(press('ArrowRight').defaultPrevented).toBe(true);

    expect(spies.goForward).toHaveBeenCalledTimes(2);
    expect(spies.goBack).not.toHaveBeenCalled();
  });

  it('need the Command key, and nothing else held with it', () => {
    press('[', document.body, { metaKey: false });
    press('ArrowLeft', document.body, { metaKey: false });
    press('[', document.body, { shiftKey: true });
    press('ArrowLeft', document.body, { altKey: true });
    press(']', document.body, { ctrlKey: true });

    expect(spies.goBack).not.toHaveBeenCalled();
    expect(spies.goForward).not.toHaveBeenCalled();
  });

  it('swallow the key with no entry that way, so the webview never steps its own history', async () => {
    // A step with nowhere to go resolves false (navigation's stepHistory).
    spies.goBack.mockResolvedValue(false);
    spies.goForward.mockResolvedValue(false);

    const back = press('[');
    const forward = press(']');
    const left = press('ArrowLeft');
    const right = press('ArrowRight');
    await Promise.all(
      [...spies.goBack.mock.results, ...spies.goForward.mock.results].map((r) => r.value)
    );

    // Asked, did nothing, and the browser's default never ran.
    expect(spies.goBack).toHaveBeenCalledTimes(2);
    expect(spies.goForward).toHaveBeenCalledTimes(2);
    expect([back, forward, left, right].every((event) => event.defaultPrevented)).toBe(true);
  });

  it('leave the keys to a text field, where ⌘← and ⌘→ move the caret', () => {
    const input = document.createElement('input');
    const textarea = document.createElement('textarea');
    const select = document.createElement('select');
    const editable = document.createElement('div');
    // jsdom has no layout, so it does not derive isContentEditable.
    Object.defineProperty(editable, 'isContentEditable', { value: true });
    document.body.append(input, textarea, select, editable);

    for (const field of [input, textarea, select, editable]) {
      for (const key of ['[', ']', 'ArrowLeft', 'ArrowRight']) {
        expect(press(key, field).defaultPrevented).toBe(false);
      }
    }

    expect(spies.goBack).not.toHaveBeenCalled();
    expect(spies.goForward).not.toHaveBeenCalled();
  });

  it('work from a control that is not a text field', () => {
    const button = document.createElement('button');
    document.body.append(button);

    press('[', button);

    expect(spies.goBack).toHaveBeenCalledTimes(1);
  });

  it('follow a rebind: the new key steps Back and the old ones no longer do', async () => {
    await updateShortcutBinding(GO_BACK_SHORTCUT_ID, ['b'], { meta: true, shift: true });

    expect(press('[').defaultPrevented).toBe(false);
    expect(press('ArrowLeft').defaultPrevented).toBe(false);
    expect(spies.goBack).not.toHaveBeenCalled();

    expect(press('b', document.body, { shiftKey: true }).defaultPrevented).toBe(true);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    // Forward is its own binding.
    press(']');
    expect(spies.goForward).toHaveBeenCalledTimes(1);
  });

  it('do nothing once withdrawn', () => {
    unregister();

    expect(press('[').defaultPrevented).toBe(false);
    expect(spies.goBack).not.toHaveBeenCalled();
    unregister = () => {};
  });
});
