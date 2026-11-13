import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import DialogBesideToaster from './testStubs/DialogBesideToaster.svelte';
import DialogWithForm from './testStubs/DialogWithForm.svelte';
import { reactiveProps } from '$lib/features/sessions/testStubs/reactiveProps.svelte';

// bits-ui closes a dialog on a pointer press outside its content. The app's
// toaster is mounted at the root, beside the dialog, so a press on a toast's
// Undo or close button would count: dialog-content keeps those presses from
// the dismissal, while a press on the page behind still closes the dialog.

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

/** The dismissal is debounced 10 ms after the press; let it, then render. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 30));
  await tick();
  flushSync();
}

async function render(onInteractOutside?: (event: PointerEvent) => void) {
  const props = reactiveProps({ open: true, onInteractOutside });
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(DialogBesideToaster, { target, props });
  flushSync();
  await settle();
  const content = document.querySelector<HTMLElement>('[data-slot="dialog-content"]');
  if (!content) throw new Error('dialog content not rendered');
  // jsdom lays nothing out; the dialog takes a box so a press can be outside it.
  content.getBoundingClientRect = () =>
    ({ left: 100, top: 100, right: 500, bottom: 500, width: 400, height: 400 }) as DOMRect;
  return props;
}

/** A mouse press on `el`, landing outside the dialog's box. */
function press(el: Element) {
  const init = { bubbles: true, cancelable: true, clientX: 700, clientY: 700 };
  el.dispatchEvent(new (window.PointerEvent ?? MouseEvent)('pointerdown', init));
  el.dispatchEvent(new MouseEvent('pointerup', init));
  el.dispatchEvent(new MouseEvent('click', init));
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
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  target.remove();
  vi.unstubAllGlobals();
});

describe('Dialog.Content outside interactions', () => {
  it('closes on a press on the page behind', async () => {
    const props = await render();

    press(document.querySelector('[data-testid="page-button"]')!);
    await settle();

    expect(props.open).toBe(false);
  });

  it("stays open through a press on a toast's button", async () => {
    const props = await render();

    press(document.querySelector('[data-testid="toast-undo"]')!);
    await settle();

    expect(props.open).toBe(true);
    expect(document.querySelector('[data-slot="dialog-content"]')).not.toBeNull();
  });

  it("runs the caller's handler first, which can keep the dialog open itself", async () => {
    const seen: string[] = [];
    const props = await render((event) => {
      seen.push((event.target as HTMLElement).dataset.testid ?? '');
      event.preventDefault();
    });

    press(document.querySelector('[data-testid="page-button"]')!);
    await settle();

    expect(seen).toEqual(['page-button']);
    expect(props.open).toBe(true);
  });
});

// A dialog with a form submits it on a plain Return from anywhere inside
// that has no use of its own for the key. To hear the key it takes the focus
// itself as it opens, unless something inside already has it. Dialogs with
// nothing for Return to submit are left exactly as they were: a viewer, a
// form around a multiline editor (Cmd+Return submits those), and any dialog
// whose caller handles the open focus.

type FormFixtureProps = NonNullable<Parameters<typeof DialogWithForm>[1]>;

/** Open the form fixture from a focused button on the page behind it. */
async function openFromPage(overrides: Partial<FormFixtureProps> = {}) {
  const props = reactiveProps({
    open: false,
    onSubmit: vi.fn(),
    onCancel: vi.fn(),
    ...overrides,
  });
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(DialogWithForm, { target, props });
  flushSync();
  pageButton().focus();
  props.open = true;
  flushSync();
  await settle();
  return props;
}

function pageButton(): HTMLElement {
  return document.querySelector<HTMLElement>('[data-testid="page-button"]')!;
}

function dialogContent(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-slot="dialog-content"]');
  if (!el) throw new Error('dialog content not rendered');
  return el;
}

function part(testid: string): HTMLElement {
  const el = dialogContent().querySelector<HTMLElement>(`[data-testid="${testid}"]`);
  if (!el) throw new Error(`no ${testid} in the dialog`);
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

describe('Dialog.Content open focus', () => {
  it('takes the focus itself when it has a form to submit', async () => {
    await openFromPage();

    expect(document.activeElement).toBe(dialogContent());
  });

  it('takes it while the submit button is still disabled', async () => {
    await openFromPage({ submitDisabled: true });

    expect(document.activeElement).toBe(dialogContent());
  });

  it('leaves the focus on a field the dialog focuses itself', async () => {
    await openFromPage({ focusField: true });

    expect(document.activeElement).toBe(part('field'));
  });

  it('leaves the focus where it was for a dialog with nothing to submit', async () => {
    await openFromPage({ body: 'viewer' });

    expect(document.activeElement).toBe(pageButton());
  });

  it.each(['textarea', 'prompt'] as const)(
    'leaves the focus where it was for a form with a multiline editor (%s)',
    async (body) => {
      await openFromPage({ body });

      expect(document.activeElement).toBe(pageButton());
    }
  );

  it('leaves the focus where it was when the caller says none', async () => {
    await openFromPage({ defaultAction: 'none' });

    expect(document.activeElement).toBe(pageButton());
  });

  it("leaves the focus to the caller's handler, which can keep it unset", async () => {
    const onOpenAutoFocus = vi.fn((event: Event) => event.preventDefault());
    await openFromPage({ onOpenAutoFocus });

    expect(onOpenAutoFocus).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(pageButton());
  });

  it("leaves the focus where the caller's handler puts it", async () => {
    await openFromPage({
      onOpenAutoFocus: (event) => {
        event.preventDefault();
        part('cancel').focus();
      },
    });

    expect(document.activeElement).toBe(part('cancel'));
  });

  it('takes no focus once the dialog has closed again', async () => {
    const props = reactiveProps({ open: false });
    target = document.createElement('div');
    document.body.appendChild(target);
    component = mount(DialogWithForm, { target, props });
    flushSync();
    pageButton().focus();

    props.open = true;
    flushSync();
    props.open = false;
    flushSync();
    await settle();

    expect(document.activeElement).toBe(pageButton());
  });
});

describe('Dialog.Content Return key', () => {
  let windowKeydown: ReturnType<typeof vi.fn<(event: KeyboardEvent) => void>>;

  beforeEach(() => {
    windowKeydown = vi.fn();
    window.addEventListener('keydown', windowKeydown);
  });

  afterEach(() => {
    window.removeEventListener('keydown', windowKeydown);
  });

  it('submits the form on Return as it opens, with focus on the dialog', async () => {
    const props = await openFromPage();

    const event = pressReturn(document.activeElement!);

    expect(props.onSubmit).toHaveBeenCalledTimes(1);
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
  });

  it('submits on Return aimed at text inside the dialog', async () => {
    const props = await openFromPage();

    pressReturn(part('text'));

    expect(props.onSubmit).toHaveBeenCalledTimes(1);
  });

  it('submits on Return on the dialog after its own field had the focus', async () => {
    const props = await openFromPage({ focusField: true });
    // Where WebKit puts the focus after a click on a picker row or a label.
    dialogContent().focus();

    pressReturn(document.activeElement!);

    expect(props.onSubmit).toHaveBeenCalledTimes(1);
  });

  it('keeps a handled Return from listeners on the window', async () => {
    await openFromPage();

    pressReturn(document.activeElement!);

    expect(windowKeydown).not.toHaveBeenCalled();
  });

  it('does not submit while the submit button is disabled, and does once it is not', async () => {
    const props = await openFromPage({ submitDisabled: true });

    const event = pressReturn(document.activeElement!);
    expect(props.onSubmit).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);

    props.submitDisabled = false;
    flushSync();
    pressReturn(document.activeElement!);
    expect(props.onSubmit).toHaveBeenCalledTimes(1);
  });

  it('does not add a submit to a Return in a field, which submits on its own', async () => {
    const props = await openFromPage({ focusField: true });

    // The browser turns this keydown into the form's submit; the dialog
    // must not add one.
    const event = pressReturn(document.activeElement!);

    expect(props.onSubmit).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it.each(['cancel', 'row', 'submit'] as const)(
    'leaves a Return on the focused %s button to that button',
    async (testid) => {
      const props = await openFromPage();
      part(testid).focus();

      const event = pressReturn(document.activeElement!);

      expect(props.onSubmit).not.toHaveBeenCalled();
      expect(event.defaultPrevented).toBe(false);
    }
  );

  it('does not submit on a Return that commits an IME composition', async () => {
    const props = await openFromPage();

    pressReturn(document.activeElement!, { isComposing: true });
    pressReturn(document.activeElement!, { keyCode: 229 });

    expect(props.onSubmit).not.toHaveBeenCalled();
  });

  it('does not submit on a held Return repeating', async () => {
    const props = await openFromPage();

    pressReturn(document.activeElement!, { repeat: true });

    expect(props.onSubmit).not.toHaveBeenCalled();
  });

  it.each(['metaKey', 'ctrlKey', 'altKey', 'shiftKey'] as const)(
    'does not submit on %s+Return',
    async (modifier) => {
      const props = await openFromPage();

      pressReturn(document.activeElement!, { [modifier]: true });

      expect(props.onSubmit).not.toHaveBeenCalled();
    }
  );

  it.each(['textarea', 'prompt'] as const)(
    'never submits a form with a multiline editor on a plain Return (%s)',
    async (body) => {
      const props = await openFromPage({ body });

      // In the editor, on the dialog (after a click on its header) and on
      // its text: untouched, as before there was any Return handling.
      const events = [part('editor'), dialogContent(), part('text')].map((el) => pressReturn(el));

      expect(props.onSubmit).not.toHaveBeenCalled();
      expect(events.map((event) => event.defaultPrevented)).toEqual([false, false, false]);
      expect(windowKeydown).toHaveBeenCalledTimes(3);
    }
  );

  it('leaves Return alone when the caller says none', async () => {
    const props = await openFromPage({ defaultAction: 'none' });

    const event = pressReturn(dialogContent());

    expect(props.onSubmit).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
    expect(windowKeydown).toHaveBeenCalledTimes(1);
  });

  it('submits a multiline form from the dialog when the caller says submit, never from its editor', async () => {
    const props = await openFromPage({ body: 'textarea', defaultAction: 'submit' });
    expect(document.activeElement).toBe(dialogContent());

    pressReturn(part('editor'));
    expect(props.onSubmit).not.toHaveBeenCalled();

    pressReturn(dialogContent());
    expect(props.onSubmit).toHaveBeenCalledTimes(1);
  });

  it('leaves Return alone in a dialog with nothing to submit', async () => {
    const props = await openFromPage({ body: 'viewer' });

    const event = pressReturn(part('text'));

    expect(props.open).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    expect(windowKeydown).toHaveBeenCalledTimes(1);
  });

  it("runs the caller's keydown handler first, which can take the Return itself", async () => {
    const seen: string[] = [];
    const props = await openFromPage({
      onkeydown: (event) => {
        seen.push(event.key);
        event.preventDefault();
      },
    });

    pressReturn(document.activeElement!);

    expect(seen).toEqual(['Enter']);
    expect(props.onSubmit).not.toHaveBeenCalled();
  });

  it('still closes on Escape', async () => {
    const props = await openFromPage();

    document.activeElement!.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    );
    await settle();

    expect(props.onSubmit).not.toHaveBeenCalled();
    expect(props.open).toBe(false);
  });
});
