import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch } from '../../types';

// The new-session dialog's Return key, with the real dialog chrome
// (NewSessionModal.component.test.ts stubs it). Its prompt is multiline, so
// Cmd/Ctrl+Return starts the session and a plain Return never does, although
// the dialog holds a form with a submit button like the ones a plain Return
// submits (see dialog-content).
vi.mock('./HashtagInput.svelte', () => import('./testStubs/PromptInputStub.svelte'));
vi.mock('./ImageAttachment.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('../agents/AcpConfigPicker.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('../branches/dragDrop', () => ({ subscribeDragDrop: () => () => {} }));

import NewSessionModal from './NewSessionModal.svelte';

const branch = {
  id: 'branch-1',
  projectId: 'project-1',
  branchName: 'feature-branch',
  worktreePath: '/tmp/worktree',
} as unknown as Branch;

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

/** Let the draft load, the prompt's focus and the dialog's open frame run. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 30));
  for (let i = 0; i < 8; i++) await tick();
  flushSync();
}

async function openModal() {
  const onSubmit = vi.fn();
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(NewSessionModal, {
    target,
    props: {
      open: true,
      branch,
      mode: 'commit',
      initialPrompt: 'Tidy the imports',
      hashtagItems: [],
      onClose: () => {},
      onSubmit,
    },
  });
  flushSync();
  await settle();
  return onSubmit;
}

function dialogContent(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-slot="dialog-content"]');
  if (!el) throw new Error('new session dialog not rendered');
  return el;
}

function prompt(): HTMLElement {
  return dialogContent().querySelector<HTMLElement>('[data-testid="prompt-input"]')!;
}

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
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  target.remove();
  vi.unstubAllGlobals();
});

describe('NewSessionModal Return key', () => {
  it('opens with focus on the prompt, not on the dialog', async () => {
    await openModal();

    expect(document.activeElement).toBe(prompt());
  });

  it('can start the session, so a Return has something it could wrongly press', async () => {
    await openModal();

    const submit = dialogContent().querySelector<HTMLButtonElement>('button[type="submit"]')!;
    expect(submit.disabled).toBe(false);
  });

  it('does not start the session on a plain Return in the prompt', async () => {
    const onSubmit = await openModal();

    const event = pressReturn(prompt());

    expect(onSubmit).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it('does not start the session on a plain Return on the dialog itself', async () => {
    const onSubmit = await openModal();
    // Where WebKit puts the focus after a click on the header or a button.
    dialogContent().focus();

    const event = pressReturn(document.activeElement!);

    expect(onSubmit).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it('still starts the session on Ctrl+Return', async () => {
    const onSubmit = await openModal();

    pressReturn(prompt(), { ctrlKey: true });

    expect(onSubmit).toHaveBeenCalledTimes(1);
  });
});
