import { flushSync, mount, tick, unmount, type ComponentProps } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch } from '../../types';

// Heavy leaf components are stubbed out: the tests exercise the modal's draft
// effect wiring, not the editor/dialog/picker internals.
vi.mock('$lib/components/ui/dialog', async () => {
  const { default: Passthrough } = await import('./testStubs/PassthroughStub.svelte');
  return { Root: Passthrough, Content: Passthrough, Title: Passthrough };
});
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

function mountModal(props: Partial<ComponentProps<typeof NewSessionModal>>) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  return mount(NewSessionModal, {
    target,
    props: {
      open: true,
      branch,
      mode: 'commit',
      hashtagItems: [],
      onClose: () => {},
      onSubmit: () => {},
      ...props,
    },
  });
}

/** Let the modal's async draft loads and effect flushes run to completion. */
async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

function typeInPrompt(text: string) {
  const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
  if (!input) throw new Error('prompt input not rendered');
  input.textContent = text;
  input.dispatchEvent(new InputEvent('input', { bubbles: true }));
  flushSync();
}

/** ⌘/Ctrl+1/2/3 mode-switch shortcut (jsdom is non-Mac, so Ctrl). */
function pressModeShortcut(key: string) {
  window.dispatchEvent(new KeyboardEvent('keydown', { key, ctrlKey: true, cancelable: true }));
  flushSync();
}

describe('NewSessionModal draft autosave', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('keeps firing onDraftChange for keystrokes after a mode switch', async () => {
    // Regression: the draft-save $effect used to guard on non-reactive locals
    // before reading any signals; a run during a mode switch then read zero
    // reactive values, Svelte truncated its dependency list to empty, and
    // autosave was dead for the rest of the modal's life.
    const onDraftChange = vi.fn();
    const modal = mountModal({
      onDraftChange,
      loadDraftForMode: vi.fn(async () => null),
    });
    flushSync();
    await settle();

    pressModeShortcut('1'); // commit → note
    await settle();
    expect(onDraftChange).toHaveBeenCalledWith(expect.objectContaining({ mode: 'note' }));

    onDraftChange.mockClear();
    typeInPrompt('typed after switch');
    await settle();
    expect(onDraftChange).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: 'typed after switch', mode: 'note' })
    );

    unmount(modal);
  });

  it('populates the composer from the stored draft without blocking the open', async () => {
    const onDraftChange = vi.fn();
    const modal = mountModal({
      onDraftChange,
      loadDraftForMode: vi.fn(async () => 'stored draft'),
    });
    flushSync();
    // The modal mounts before the store read resolves…
    expect(document.querySelector('[data-testid="prompt-input"]')).not.toBeNull();

    // …and the stored draft lands afterwards.
    await settle();
    expect(onDraftChange).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: 'stored draft', mode: 'commit' })
    );

    unmount(modal);
  });

  it('lets text typed during the initial draft load win, and still saves it', async () => {
    let resolveLoad!: (value: string | null) => void;
    const onDraftChange = vi.fn();
    const modal = mountModal({
      onDraftChange,
      loadDraftForMode: vi.fn(() => new Promise<string | null>((r) => (resolveLoad = r))),
    });
    flushSync();
    await settle();
    // The save gate stays closed while the stored draft is loading, so the
    // stale seed is never written back over a newer pending store write…
    expect(onDraftChange).not.toHaveBeenCalled();

    typeInPrompt('typed during load');
    await settle();
    expect(onDraftChange).not.toHaveBeenCalled();

    resolveLoad('stored draft');
    await settle();
    // …and once the load lands, the typed text is preserved over the stored
    // draft and reaches onDraftChange so it still gets persisted.
    const prompts = onDraftChange.mock.calls.map(([draft]) => (draft as { prompt: string }).prompt);
    expect(prompts).toContain('typed during load');
    expect(prompts).not.toContain('stored draft');

    unmount(modal);
  });
});
