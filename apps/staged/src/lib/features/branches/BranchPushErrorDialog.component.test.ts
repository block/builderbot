import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import BranchPushErrorDialog from './BranchPushErrorDialog.svelte';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

// The push failure alert, with the real alert dialog chrome, which decides
// where focus lands and which keys reach the buttons. Retry repeats the
// failed push as it was asked for, so on a failed force push it force pushes
// again: Return must not do that.

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

/** bits-ui focuses the dialog on a 0 ms timer after mount; let it, then render. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 30));
  await tick();
  flushSync();
}

async function render(force: boolean) {
  const props = reactiveProps({
    open: true,
    force,
    error: 'remote: rejected' as string | null,
    onRetry: vi.fn(),
    onCancel: vi.fn(),
  });
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(BranchPushErrorDialog, { target, props });
  flushSync();
  await settle();
  return props;
}

function keydown(el: Element, init: KeyboardEventInit = {}) {
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

function buttonNamed(name: string): HTMLButtonElement {
  const found = [...document.querySelectorAll('button')].find(
    (b) => b.textContent?.trim() === name
  );
  if (!found) throw new Error(`no "${name}" button`);
  return found;
}

function dialog(): HTMLElement {
  const found = document.querySelector<HTMLElement>('[role="alertdialog"]');
  if (!found) throw new Error('no alert dialog');
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

describe('BranchPushErrorDialog', () => {
  describe('after a plain push failed', () => {
    it('shows the error under a Push Failed title', async () => {
      await render(false);

      expect(dialog().textContent).toContain('Push Failed');
      expect(dialog().textContent).not.toContain('Force Push Failed');
      expect(dialog().textContent).toContain('remote: rejected');
    });

    it('retries on Return as it opens, with focus on the dialog', async () => {
      const props = await render(false);
      expect(document.activeElement).toBe(dialog());

      const event = keydown(document.activeElement!);

      expect(props.onRetry).toHaveBeenCalledTimes(1);
      expect(props.onCancel).not.toHaveBeenCalled();
      expect(event.defaultPrevented).toBe(true);
    });

    it('only cancels on Return on a focused Cancel', async () => {
      const props = await render(false);
      buttonNamed('Cancel').focus();

      keydown(document.activeElement!);

      expect(props.onRetry).not.toHaveBeenCalled();
    });
  });

  describe('after a force push failed', () => {
    it('says so in the title', async () => {
      await render(true);

      expect(dialog().textContent).toContain('Force Push Failed');
    });

    it('does not retry on Return as it opens, with focus on the dialog', async () => {
      const props = await render(true);
      expect(document.activeElement).toBe(dialog());

      const event = keydown(document.activeElement!);

      expect(props.onRetry).not.toHaveBeenCalled();
      expect(props.onCancel).not.toHaveBeenCalled();
      // Left alone, not swallowed, and the dialog stays up.
      expect(event.defaultPrevented).toBe(false);
      expect(props.open).toBe(true);
      expect(document.querySelector('[role="alertdialog"]')).not.toBeNull();
    });

    it('does not retry on Return on the error text', async () => {
      const props = await render(true);

      keydown(dialog().querySelector('[data-slot="alert-dialog-description"]')!);

      expect(props.onRetry).not.toHaveBeenCalled();
    });

    it('still retries on a click on Retry', async () => {
      const props = await render(true);

      buttonNamed('Retry').click();
      flushSync();

      expect(props.onRetry).toHaveBeenCalledTimes(1);
      expect(props.onCancel).not.toHaveBeenCalled();
    });

    it('leaves a Return on a focused Retry to the button, which presses itself', async () => {
      const props = await render(true);
      buttonNamed('Retry').focus();

      // The dialog leaves a focused button's Return to the button.
      const event = keydown(document.activeElement!);

      expect(event.defaultPrevented).toBe(false);
      expect(props.onRetry).not.toHaveBeenCalled();
    });

    it('goes back to retrying on Return once the failed push is a plain one', async () => {
      const props = await render(true);
      props.force = false;
      flushSync();

      keydown(dialog());

      expect(props.onRetry).toHaveBeenCalledTimes(1);
    });
  });

  it('clears the failed push on Cancel', async () => {
    const props = await render(true);

    buttonNamed('Cancel').click();
    await settle();

    expect(props.onCancel).toHaveBeenCalledTimes(1);
    expect(props.onRetry).not.toHaveBeenCalled();
  });

  it('only hides on Escape, keeping the failed push to reopen', async () => {
    const props = await render(true);

    keydown(dialog(), { key: 'Escape' });
    await settle();

    expect(props.open).toBe(false);
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(props.onRetry).not.toHaveBeenCalled();
  });
});
