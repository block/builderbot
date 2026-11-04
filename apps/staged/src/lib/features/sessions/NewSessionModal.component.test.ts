import { flushSync, mount, tick, unmount, type ComponentProps } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, BranchSessionType } from '../../types';

// Heavy leaf components are stubbed out: the tests exercise the modal's draft
// effect wiring, not the editor/dialog/picker internals.
vi.mock('$lib/components/ui/dialog', () =>
  import('./testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
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

/** Dismiss the modal via the header close button (same path as Escape). */
function clickCloseButton() {
  const btn = document.querySelector<HTMLButtonElement>('[aria-label="Close"]');
  if (!btn) throw new Error('close button not rendered');
  btn.click();
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

  it('appends text typed during the initial load to the stored draft and saves both', async () => {
    // Regression: the initial load skipped the stored draft entirely once the
    // user had typed, then the reopened save effect overwrote the stored
    // draft with just the typed text — the one load path that still lost a
    // stored draft to a keystroke landing in the load window.
    let resolveLoad!: (value: string | null) => void;
    const onDraftChange = vi.fn();
    const modal = mountModal({
      initialPrompt: '',
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
    // …and once the load lands, the stored draft comes first with the typed
    // text after it, and that combined value is what gets persisted.
    const prompts = onDraftChange.mock.calls.map(([draft]) => (draft as { prompt: string }).prompt);
    expect(prompts.length).toBeGreaterThan(0);
    expect(prompts[prompts.length - 1]).toBe('stored draft\ntyped during load');
    expect(prompts).not.toContain('typed during load');

    unmount(modal);
  });

  it('lets text typed during the initial load win when there is no stored draft', async () => {
    let resolveLoad!: (value: string | null) => void;
    const onDraftChange = vi.fn();
    const modal = mountModal({
      initialPrompt: 'prefill',
      prefilled: true,
      onDraftChange,
      loadDraftForMode: vi.fn(() => new Promise<string | null>((r) => (resolveLoad = r))),
    });
    flushSync();
    await settle();

    typeInPrompt('typed over prefill');
    await settle();

    resolveLoad(null);
    await settle();
    expect(onDraftChange).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: 'typed over prefill', mode: 'commit' })
    );

    unmount(modal);
  });

  it('keeps a replacement typed over the prefill during the initial load whole', async () => {
    // A prefilled open selects all, so the natural gesture in the load window
    // is typing a replacement over the prefill. Diffing that against the seed
    // drops boundary characters the replacement shares with the prefill
    // (here the trailing 's'); a prompt that no longer contains the seed is
    // wholly typed and must carry over intact.
    let resolveLoad!: (value: string | null) => void;
    const onDraftChange = vi.fn();
    const modal = mountModal({
      initialPrompt: 'Commit the changes',
      prefilled: true,
      onDraftChange,
      loadDraftForMode: vi.fn(() => new Promise<string | null>((r) => (resolveLoad = r))),
    });
    flushSync();
    await settle();

    typeInPrompt('add tests');
    await settle();

    resolveLoad('stored draft');
    await settle();
    const prompts = onDraftChange.mock.calls.map(([draft]) => (draft as { prompt: string }).prompt);
    expect(prompts.length).toBeGreaterThan(0);
    expect(prompts[prompts.length - 1]).toBe('stored draft\nadd tests');

    unmount(modal);
  });

  it('keeps edits made during the initial load when the seed already is the stored draft', async () => {
    // The caller seeds the modal with the draft it holds in memory, which is
    // normally what the store holds too; edits typed during the load then
    // sit on top of the stored draft and must not be re-appended to it.
    let resolveLoad!: (value: string | null) => void;
    const onDraftChange = vi.fn();
    const modal = mountModal({
      initialPrompt: 'stored draft',
      onDraftChange,
      loadDraftForMode: vi.fn(() => new Promise<string | null>((r) => (resolveLoad = r))),
    });
    flushSync();
    await settle();

    typeInPrompt('stored draft plus');
    await settle();

    resolveLoad('stored draft');
    await settle();
    const prompts = onDraftChange.mock.calls.map(([draft]) => (draft as { prompt: string }).prompt);
    expect(prompts.length).toBeGreaterThan(0);
    expect(prompts.every((p) => p === 'stored draft plus')).toBe(true);

    unmount(modal);
  });

  it('keeps text typed during a mode-switch load and appends it to the stored draft', async () => {
    // Regression: the mode-switch load overwrote `prompt` wholesale when it
    // landed, so keystrokes typed between ⌘1/⌘2/⌘3 and the load resolving
    // were dropped from both modes — the save effect is gated during the
    // switch, and the outgoing save had already been snapshotted.
    let resolveNoteLoad!: (value: string | null) => void;
    const onDraftChange = vi.fn();
    const modal = mountModal({
      onDraftChange,
      loadDraftForMode: vi.fn((mode: BranchSessionType) => {
        if (mode === 'note') return new Promise<string | null>((r) => (resolveNoteLoad = r));
        return Promise.resolve(null);
      }),
    });
    flushSync();
    await settle();

    typeInPrompt('commit text');
    await settle();

    pressModeShortcut('1'); // commit → note; note's draft load stays pending
    await settle();
    onDraftChange.mockClear();

    // The outgoing commit text is still on screen; the user types after it.
    typeInPrompt('commit text typed mid-switch');
    await settle();

    resolveNoteLoad('note stored draft');
    await settle();

    // The typed delta belongs to note mode: it lands after the stored note
    // draft, the commit text does not come along, and the result is saved
    // under note's key.
    const noteSaves = onDraftChange.mock.calls
      .filter(([draft]) => (draft as { mode: string }).mode === 'note')
      .map(([draft]) => (draft as { prompt: string }).prompt);
    expect(noteSaves.length).toBeGreaterThan(0);
    const final = noteSaves[noteSaves.length - 1];
    expect(final).toContain('note stored draft');
    expect(final).toContain('typed mid-switch');
    expect(final).not.toContain('commit text');
    expect(final.indexOf('note stored draft')).toBeLessThan(final.indexOf('typed mid-switch'));

    unmount(modal);
  });

  it('keeps text typed during a mode-switch load when the new mode has no stored draft', async () => {
    let resolveNoteLoad!: (value: string | null) => void;
    const onDraftChange = vi.fn();
    const modal = mountModal({
      onDraftChange,
      loadDraftForMode: vi.fn((mode: BranchSessionType) => {
        if (mode === 'note') return new Promise<string | null>((r) => (resolveNoteLoad = r));
        return Promise.resolve(null);
      }),
    });
    flushSync();
    await settle();

    pressModeShortcut('1'); // commit → note; note's draft load stays pending
    await settle();
    onDraftChange.mockClear();

    typeInPrompt('typed mid-switch');
    await settle();

    resolveNoteLoad(null);
    await settle();

    expect(onDraftChange).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: 'typed mid-switch', mode: 'note' })
    );

    unmount(modal);
  });

  it('files the draft under its owner mode when closed during a mode switch', async () => {
    const pendingNoteLoads: Array<(value: string | null) => void> = [];
    const onClose = vi.fn();
    const onDraftChange = vi.fn();
    const modal = mountModal({
      onClose,
      onDraftChange,
      loadDraftForMode: vi.fn((mode: BranchSessionType) => {
        if (mode === 'note') return new Promise<string | null>((r) => pendingNoteLoads.push(r));
        return Promise.resolve(null);
      }),
    });
    flushSync();
    await settle();

    typeInPrompt('commit text');
    await settle();

    pressModeShortcut('1'); // commit → note; note's draft load stays pending
    await settle();

    // Dismissing mid-switch: the text on screen still belongs to commit mode,
    // so the close must pair it with commit — filing it under the
    // half-switched-to note mode would clobber note's stored draft.
    clickCloseButton();
    expect(onClose).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: 'commit text', mode: 'commit' })
    );

    // The superseded switch must not fire another draft change after close.
    onDraftChange.mockClear();
    pendingNoteLoads.forEach((resolve) => resolve('note stored draft'));
    await settle();
    expect(onDraftChange).not.toHaveBeenCalled();

    unmount(modal);
  });

  it('saves text typed during the initial load when a mode switch interrupts it', async () => {
    let resolveInitialLoad!: (value: string | null) => void;
    const onDraftChange = vi.fn();
    const modal = mountModal({
      onDraftChange,
      loadDraftForMode: vi.fn((mode: BranchSessionType) => {
        if (mode === 'commit') {
          return new Promise<string | null>((r) => (resolveInitialLoad = r));
        }
        return Promise.resolve(null);
      }),
    });
    flushSync();
    await settle();

    typeInPrompt('typed during load');
    await settle();

    // Switching modes before the initial load resolves: unlike a switch-in-
    // flight, no prior save exists for this text, so the outgoing save must
    // still run — filed under commit, the mode the prompt belongs to.
    pressModeShortcut('1'); // commit → note
    await settle();
    expect(onDraftChange).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: 'typed during load', mode: 'commit' }),
      expect.objectContaining({ immediate: true })
    );

    // The superseded initial load must not clobber the new mode's prompt.
    resolveInitialLoad('stale stored commit draft');
    await settle();
    const prompts = onDraftChange.mock.calls.map(([draft]) => (draft as { prompt: string }).prompt);
    expect(prompts).not.toContain('stale stored commit draft');

    unmount(modal);
  });
});
