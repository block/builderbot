import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import AlertDialogWithButtons from './testStubs/AlertDialogWithButtons.svelte';
import { reactiveProps } from '$lib/features/sessions/testStubs/reactiveProps.svelte';

// An alert dialog opens with focus on its own container (bits-ui puts it
// there, not on a button), so nothing native answers Return. The content
// presses the default button itself: the Action, unless it is destructive.
// The real chrome is mounted here; the feature tests stub it.

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;
let windowKeydown: ReturnType<typeof vi.fn<(event: KeyboardEvent) => void>>;

/** bits-ui focuses the container on a 0 ms timer after mount; let it, then render. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 30));
  await tick();
  flushSync();
}

type FixtureProps = NonNullable<Parameters<typeof AlertDialogWithButtons>[1]>;

async function render(overrides: Partial<FixtureProps> = {}) {
  const props = reactiveProps({
    open: true,
    onConfirm: vi.fn(),
    onCancel: vi.fn(),
    onOpenChange: vi.fn(),
    ...overrides,
  });
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(AlertDialogWithButtons, { target, props });
  flushSync();
  await settle();
  return props;
}

function content(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-slot="alert-dialog-content"]');
  if (!el) throw new Error('alert dialog content not rendered');
  return el;
}

function button(slot: 'action' | 'cancel'): HTMLButtonElement {
  const el = content().querySelector<HTMLButtonElement>(`[data-slot="alert-dialog-${slot}"]`);
  if (!el) throw new Error(`alert dialog ${slot} not rendered`);
  return el;
}

/** A Return keydown on `el`; returns the event so a test can see who took it. */
function pressReturn(el: Element, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent('keydown', {
    key: 'Enter',
    bubbles: true,
    cancelable: true,
    ...init,
  });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

beforeEach(() => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    }
  );
  windowKeydown = vi.fn();
  window.addEventListener('keydown', windowKeydown);
});

afterEach(() => {
  window.removeEventListener('keydown', windowKeydown);
  if (component) unmount(component);
  component = null;
  target.remove();
  vi.unstubAllGlobals();
});

describe('AlertDialog.Content Return key', () => {
  it('opens with focus on the dialog itself, not on a button', async () => {
    await render();

    expect(document.activeElement).toBe(content());
    expect(content().getAttribute('role')).toBe('alertdialog');
  });

  it('confirms on Return while focus is on the dialog', async () => {
    const props = await render();

    const event = pressReturn(document.activeElement!);

    expect(props.onConfirm).toHaveBeenCalledTimes(1);
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
  });

  it('confirms the same way when focus started on a button behind the dialog', async () => {
    target = document.createElement('div');
    document.body.appendChild(target);
    const props = reactiveProps({ open: false, onConfirm: vi.fn(), onCancel: vi.fn() });
    component = mount(AlertDialogWithButtons, { target, props });
    flushSync();
    document.querySelector<HTMLElement>('[data-testid="page-button"]')!.focus();

    props.open = true;
    flushSync();
    await settle();
    expect(document.activeElement).toBe(content());
    pressReturn(document.activeElement!);

    expect(props.onConfirm).toHaveBeenCalledTimes(1);
  });

  it('confirms on Return aimed at text inside the dialog', async () => {
    const props = await render();

    pressReturn(content().querySelector('[data-testid="description"]')!);

    expect(props.onConfirm).toHaveBeenCalledTimes(1);
  });

  it('keeps a handled Return from listeners on the window', async () => {
    await render();

    pressReturn(document.activeElement!);

    expect(windowKeydown).not.toHaveBeenCalled();
  });

  it('marks the Action with its variant', async () => {
    await render({ variant: 'destructive' });

    expect(button('action').dataset.variant).toBe('destructive');
  });

  it('leaves Return alone when the Action is destructive', async () => {
    const props = await render({ variant: 'destructive' });

    const event = pressReturn(document.activeElement!);

    expect(props.onConfirm).not.toHaveBeenCalled();
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(props.onOpenChange).not.toHaveBeenCalled();
    expect(props.open).toBe(true);
    // Untouched, as before there was any Return handling.
    expect(event.defaultPrevented).toBe(false);
    expect(windowKeydown).toHaveBeenCalledTimes(1);
  });

  it('leaves Return alone when the caller says none, whatever the variant', async () => {
    const props = await render({ defaultAction: 'none' });

    const event = pressReturn(document.activeElement!);

    expect(props.onConfirm).not.toHaveBeenCalled();
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(props.onOpenChange).not.toHaveBeenCalled();
    expect(props.open).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(windowKeydown).toHaveBeenCalledTimes(1);
  });

  it('does not confirm while the Action is disabled', async () => {
    const props = await render({ actionDisabled: true });

    const event = pressReturn(document.activeElement!);

    expect(props.onConfirm).not.toHaveBeenCalled();
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it('does not confirm on a Return that commits an IME composition', async () => {
    const props = await render();

    pressReturn(document.activeElement!, { isComposing: true });
    pressReturn(document.activeElement!, { keyCode: 229 });

    expect(props.onConfirm).not.toHaveBeenCalled();
  });

  it('does not confirm on a held Return repeating', async () => {
    const props = await render();

    pressReturn(document.activeElement!, { repeat: true });

    expect(props.onConfirm).not.toHaveBeenCalled();
  });

  it.each(['metaKey', 'ctrlKey', 'altKey', 'shiftKey'] as const)(
    'does not confirm on %s+Return',
    async (modifier) => {
      const props = await render();

      pressReturn(document.activeElement!, { [modifier]: true });

      expect(props.onConfirm).not.toHaveBeenCalled();
    }
  );

  it('only cancels on Return on a focused Cancel', async () => {
    const props = await render();
    button('cancel').focus();

    pressReturn(document.activeElement!);
    await settle();

    expect(props.onConfirm).not.toHaveBeenCalled();
    expect(props.onOpenChange).toHaveBeenCalledTimes(1);
    expect(props.onOpenChange).toHaveBeenCalledWith(false);
    expect(props.open).toBe(false);
  });

  it('does not press the Action a second time on Return on the focused Action', async () => {
    const props = await render();
    button('action').focus();

    // The browser turns this keydown into the button's own click; the
    // dialog must not add one.
    const event = pressReturn(document.activeElement!);

    expect(props.onConfirm).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it('cancels on Return when the caller makes Cancel the default', async () => {
    const props = await render({ defaultAction: 'cancel', withAction: false });

    pressReturn(document.activeElement!);
    await settle();

    expect(props.onCancel).toHaveBeenCalledTimes(1);
    expect(props.open).toBe(false);
  });

  it('does not cancel while a default Cancel is disabled', async () => {
    const props = await render({
      defaultAction: 'cancel',
      withAction: false,
      cancelDisabled: true,
    });

    pressReturn(document.activeElement!);
    await settle();

    expect(props.onCancel).not.toHaveBeenCalled();
    expect(props.open).toBe(true);
  });

  it('does nothing on Return when there is no Action to default to', async () => {
    const props = await render({ withAction: false });

    const event = pressReturn(document.activeElement!);
    await settle();

    expect(props.onCancel).not.toHaveBeenCalled();
    expect(props.open).toBe(true);
    expect(event.defaultPrevented).toBe(false);
  });

  it("runs the caller's keydown handler first, which can take the Return itself", async () => {
    const seen: string[] = [];
    const props = await render({
      onkeydown: (event) => {
        seen.push(event.key);
        event.preventDefault();
      },
    });

    pressReturn(document.activeElement!);

    expect(seen).toEqual(['Enter']);
    expect(props.onConfirm).not.toHaveBeenCalled();
  });

  it('still cancels on Escape', async () => {
    const props = await render();

    document.activeElement!.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    );
    await settle();

    expect(props.onConfirm).not.toHaveBeenCalled();
    expect(props.open).toBe(false);
  });
});
