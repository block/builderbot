import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ProjectTransferScanConfirm from './ProjectTransferScanConfirm.svelte';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

// The "Scan still running" confirmation, with the real alert dialog chrome:
// ProjectTransferDialog's own test stubs it, and the chrome is what decides
// where focus lands and which keys reach the buttons.

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

/** bits-ui focuses the dialog on a 0 ms timer after mount; let it, then render. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 30));
  await tick();
  flushSync();
}

async function render() {
  const props = reactiveProps({ open: true, onConfirm: vi.fn(), onCancel: vi.fn() });
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectTransferScanConfirm, { target, props });
  flushSync();
  await settle();
  return props;
}

function keydown(el: Element, key: string) {
  el.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  flushSync();
}

function buttonNamed(name: string): HTMLButtonElement {
  const found = [...document.querySelectorAll('button')].find(
    (b) => b.textContent?.trim() === name
  );
  if (!found) throw new Error(`no "${name}" button`);
  return found;
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

describe('ProjectTransferScanConfirm', () => {
  it('starts the transfer on Return as it opens, with focus on the dialog', async () => {
    const props = await render();
    expect(document.activeElement?.getAttribute('role')).toBe('alertdialog');

    keydown(document.activeElement!, 'Enter');

    expect(props.onConfirm).toHaveBeenCalledTimes(1);
    expect(props.onCancel).not.toHaveBeenCalled();
  });

  it('starts the transfer on a click on Transfer anyway', async () => {
    const props = await render();

    buttonNamed('Transfer anyway').click();
    flushSync();

    expect(props.onConfirm).toHaveBeenCalledTimes(1);
    expect(props.onCancel).not.toHaveBeenCalled();
  });

  it('keeps checking on Escape', async () => {
    const props = await render();

    keydown(document.activeElement!, 'Escape');
    await settle();

    expect(props.onCancel).toHaveBeenCalledTimes(1);
    expect(props.onConfirm).not.toHaveBeenCalled();
  });

  it('keeps checking, and does not transfer, on Return on a focused Keep checking', async () => {
    const props = await render();
    buttonNamed('Keep checking').focus();

    keydown(document.activeElement!, 'Enter');
    await settle();

    expect(props.onCancel).toHaveBeenCalledTimes(1);
    expect(props.onConfirm).not.toHaveBeenCalled();
  });

  it('does not transfer on a held Return repeating', async () => {
    const props = await render();

    document.activeElement!.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', repeat: true, bubbles: true, cancelable: true })
    );
    flushSync();

    expect(props.onConfirm).not.toHaveBeenCalled();
  });
});
