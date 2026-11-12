import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import DialogBesideToaster from './testStubs/DialogBesideToaster.svelte';
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
