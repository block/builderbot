import { flushSync, mount, tick, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session } from '../../types';

const { writer, loadDraftMock, saveDraftMock, toastMock, resumeSessionMock, viewportMock } =
  vi.hoisted(() => {
    let toastIds = 0;
    const toastMock = Object.assign(
      vi.fn((_message: string, _options?: unknown) => `toast-${++toastIds}`),
      { error: vi.fn(), success: vi.fn(), dismiss: vi.fn() }
    );
    return {
      writer: {
        scheduleSave: vi.fn(),
        saveNow: vi.fn(async () => {}),
        clearNow: vi.fn(async () => {}),
        flush: vi.fn(async () => {}),
        run: vi.fn(<T>(task: () => Promise<T>) => task()),
      },
      /** A stored draft: a plain string, or a whole draft entry with its blocks. */
      loadDraftMock: vi.fn(
        async (_key: string): Promise<string | { value: string; structured?: unknown } | null> =>
          null
      ),
      saveDraftMock: vi.fn(async (_key: string, _content: unknown) => {}),
      toastMock,
      /** The idle composer's send; rejects to fail one. */
      resumeSessionMock: vi.fn(async () => ({ outcome: 'resumed' })),
      /** The layout the pane reads: a desktop with a pointer unless a test says otherwise. */
      viewportMock: {
        isMobile: false,
        canSplit: true,
        showShortcutHints: true,
        hasCoarsePointer: false,
        sheetMenus: false,
        shortWindow: false,
      },
    };
  });

vi.mock('svelte-sonner', () => ({ toast: toastMock }));
vi.mock('../../shared/viewport.svelte', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/viewport.svelte')>()),
  viewport: viewportMock,
}));

// Heavy leaf components are stubbed out: the tests exercise the pane's
// reply-draft effect wiring, not the transcript/composer internals.
vi.mock('./ChatComposer.svelte', async () => {
  const { default: PromptInputStub } = await import('./testStubs/PromptInputStub.svelte');
  return { default: PromptInputStub, composerControlClass: '' };
});
vi.mock('../agents/AcpFixedConfigPicker.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./PlanCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./PipelineSteps.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./tool-calls/ToolCallCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./tool-calls/ToolCallHeader.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./NoteActivityCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock(
  '../../shared/markdown/DiagramViewerModal.svelte',
  () => import('./testStubs/EmptyStub.svelte')
);
vi.mock('../branches/dragDrop', () => ({ subscribeDragDrop: () => () => {} }));
vi.mock('../agents/agent.svelte', () => ({ agentState: { providers: [] } }));
vi.mock('../settings/preferences.svelte', () => ({
  // Read by the send control the pane hands the composer stub.
  preferences: { composerSendMode: 'queue' },
  setAcpConfigPref: vi.fn(),
}));
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../shared/draftCache', () => ({
  createDraftWriteScheduler: () => writer,
  loadDraft: (key: string) => loadDraftMock(key),
  loadDraftContent: async (key: string) => {
    const stored = await loadDraftMock(key);
    return stored === null ? null : typeof stored === 'string' ? { value: stored } : stored;
  },
  saveDraft: (key: string, content: unknown) => saveDraftMock(key, content),
}));
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  getSession: async (id: string) =>
    ({ id, status: 'completed', provider: null, workingDir: null }) as unknown as Session,
  getLatestSessionMessages: async () => ({ data: [] }),
  getOlderSessionMessages: async () => [],
  getSessionMessagesSince: async () => [],
  getSessionAcpMetadataMessages: async () => [],
  getSessionAcpMetadataMessagesSince: async () => [],
  listQueuedSessionMessages: async () => [],
  resumeSession: () => resumeSessionMock(),
}));

import SessionChatPane from './SessionChatPane.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';
import { addQuoteToReplyDraft } from './replyDrafts';
import { composerDraftFromContent, serializeComposerDraft } from './composerDraft';

function replyKey(id: string): string {
  return `draft:session-reply:${id}`;
}

function mountPane(sessionId: string) {
  const props = reactiveProps({ active: true, sessionId, hashtagItems: [] });
  const target = document.createElement('div');
  document.body.appendChild(target);
  const pane = mount(SessionChatPane, { target, props });
  return { pane, props };
}

/** Let the pane's async session/draft loads and effect flushes run to completion. */
async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

function typeInComposer(text: string) {
  const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
  if (!input) throw new Error('composer input not rendered');
  input.textContent = text;
  input.dispatchEvent(new InputEvent('input', { bubbles: true }));
  flushSync();
}

describe('SessionChatPane reply draft autosave', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    writer.scheduleSave.mockClear();
    writer.flush.mockClear();
    loadDraftMock.mockReset();
    loadDraftMock.mockImplementation(async () => null);
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  it("does not file the previous session's composer text under a session whose draft load failed", async () => {
    // Regression: on a switch A → B where B's draft load throws, the composer
    // still holds A's text; re-enabling the save gate used to fire the save
    // effect immediately and schedule that text under B's key, overwriting
    // B's stored draft.
    loadDraftMock.mockImplementation(async (key: string) => {
      if (key === replyKey('session-a')) return 'draft for A';
      throw new Error('store unavailable');
    });

    const { pane, props } = mountPane('session-a');
    flushSync();
    await settle();
    // Session A's stored draft lands in the composer and is written back
    // under A's own key (the success-path write-back that refreshes the TTL).
    expect(writer.scheduleSave).toHaveBeenCalledWith(replyKey('session-a'), 'draft for A');

    writer.scheduleSave.mockClear();
    props.sessionId = 'session-b';
    flushSync();
    await settle();
    // The failed load must not save A's leftover text under B's key.
    expect(writer.scheduleSave).not.toHaveBeenCalled();

    // The save gate is re-enabled: an edit made while B is displayed is B's
    // draft and persists under B's key.
    typeInComposer('a reply for B');
    await settle();
    expect(writer.scheduleSave).toHaveBeenCalledWith(replyKey('session-b'), 'a reply for B');

    unmount(pane);
  });

  it('saves text typed while a draft load was failing', async () => {
    let rejectLoad!: (err: Error) => void;
    loadDraftMock.mockImplementation((key: string) => {
      if (key === replyKey('session-a')) return Promise.resolve<string | null>(null);
      return new Promise<string | null>((_resolve, reject) => {
        rejectLoad = reject;
      });
    });

    const { pane, props } = mountPane('session-a');
    flushSync();
    await settle();

    props.sessionId = 'session-b';
    flushSync();
    await settle();
    writer.scheduleSave.mockClear();

    // Typed while B's load is still in flight: the gate is closed, so nothing
    // is scheduled yet…
    typeInComposer('typed during load');
    await settle();
    expect(writer.scheduleSave).not.toHaveBeenCalled();

    // …and when the load fails, the text was typed while B was displayed, so
    // it saves under B immediately rather than waiting for another keystroke.
    rejectLoad(new Error('store unavailable'));
    await settle();
    expect(writer.scheduleSave).toHaveBeenCalledWith(replyKey('session-b'), 'typed during load');

    unmount(pane);
  });
});

describe('SessionChatPane quoted replies', () => {
  const quote = { quote: 'Ship behind a flag.', headings: ['Rollout'], reply: 'Which one?' };

  beforeEach(() => {
    document.body.innerHTML = '';
    writer.scheduleSave.mockClear();
    writer.run.mockClear();
    saveDraftMock.mockClear();
    toastMock.mockClear();
    toastMock.dismiss.mockClear();
    resumeSessionMock.mockClear();
    loadDraftMock.mockReset();
    loadDraftMock.mockImplementation(async () => null);
  });

  /**
   * Mount a pane whose draft holds three quotes, then remove the middle one
   * as the composer's X would. Returns the draft as saved before the
   * removal, for undo to be measured against.
   */
  async function removeMiddleQuote(sessionId: string) {
    const { pane, props } = mountPane(sessionId);
    flushSync();
    await settle();
    await addQuoteToReplyDraft(sessionId, { ...quote, quote: 'First.' });
    await addQuoteToReplyDraft(sessionId, { ...quote, quote: 'Second.' });
    await addQuoteToReplyDraft(sessionId, { ...quote, quote: 'Third.', reply: '' });
    await settle();
    const before = composerDraftFromContent(lastSaved(replyKey(sessionId)));
    const [, second] = before.quotes;
    await removeQuote(second.id);
    expect(quoteCount()).toBe(2);
    return { pane, props, before, second };
  }

  async function removeQuote(id: string) {
    document.querySelector<HTMLElement>(`[data-testid="remove-quote-${id}"]`)!.click();
    await settle();
  }

  /**
   * Undo the newest removal, as the browser's undo reaches the composer's
   * step for it (the stub keeps the steps; see PromptInputStub.svelte).
   */
  async function undoRemoval() {
    document.querySelector<HTMLElement>('[data-testid="undo-remove-quote"]')!.click();
    await settle();
  }

  /** Whether the pane took each undo so far, oldest first. */
  function undoResults(): boolean[] {
    const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
    return (input?.dataset.undoResults ?? '')
      .split(' ')
      .filter(Boolean)
      .map((result) => result === 'true');
  }

  /** The quotes the pane reported back in the composer after an undo. */
  function restoredQuotes(): string[] {
    const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
    return (input?.dataset.restoredQuotes ?? '').split(' ').filter(Boolean);
  }

  function quoteCount(): number {
    const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
    return Number(input?.dataset.quoteCount ?? NaN);
  }

  function lastSaved(key: string) {
    const calls = writer.scheduleSave.mock.calls.filter(([savedKey]) => savedKey === key);
    return calls[calls.length - 1]?.[1];
  }

  it('takes a note quote into the open composer and saves it with the draft', async () => {
    loadDraftMock.mockImplementation(async () => 'Before the quote');
    const { pane } = mountPane('session-q');
    flushSync();
    await settle();

    await addQuoteToReplyDraft('session-q', quote);
    await settle();

    expect(quoteCount()).toBe(1);
    const saved = lastSaved(replyKey('session-q'));
    expect(saved).toMatchObject({ structured: expect.anything() });
    const draft = composerDraftFromContent(saved);
    expect(draft.quotes[0]).toMatchObject({ before: 'Before the quote', ...quote });
    expect(draft.text).toBe('');
    expect(saved.value).toBe(serializeComposerDraft(draft));

    unmount(pane);
  });

  it('removes a quote without a toast, and undo puts it back in its place', async () => {
    const { pane, before, second } = await removeMiddleQuote('session-undo');
    const removed = composerDraftFromContent(lastSaved(replyKey('session-undo')));
    expect(removed.quotes.map((q) => q.quote)).toEqual(['First.', 'Third.']);
    expect(toastMock).not.toHaveBeenCalled();

    await undoRemoval();

    expect(undoResults()).toEqual([true]);
    expect(quoteCount()).toBe(3);
    const restored = composerDraftFromContent(lastSaved(replyKey('session-undo')));
    expect(restored.quotes.map((q) => q.quote)).toEqual(['First.', 'Second.', 'Third.']);
    expect(restored.quotes[1]).toMatchObject({ id: second.id, reply: 'Which one?' });
    expect(restored).toEqual(before);
    // The composer is told once the block is back in it, to place the caret.
    expect(restoredQuotes()).toEqual([second.id]);

    // Redo is the composer removing the quote again, which undo reverses again.
    await removeQuote(second.id);
    expect(composerDraftFromContent(lastSaved(replyKey('session-undo')))).toEqual(removed);
    await undoRemoval();
    expect(composerDraftFromContent(lastSaved(replyKey('session-undo')))).toEqual(before);

    expect(toastMock).not.toHaveBeenCalled();
    expect(toastMock.error).not.toHaveBeenCalled();

    unmount(pane);
  });

  it('removes an unanswered quote the same way: no toast, and undo puts it back', async () => {
    const { pane } = mountPane('session-unanswered');
    flushSync();
    await settle();
    await addQuoteToReplyDraft('session-unanswered', { ...quote, reply: '' });
    await settle();
    const before = composerDraftFromContent(lastSaved(replyKey('session-unanswered')));

    await removeQuote(before.quotes[0].id);
    expect(quoteCount()).toBe(0);
    expect(toastMock).not.toHaveBeenCalled();

    await undoRemoval();
    expect(quoteCount()).toBe(1);
    expect(composerDraftFromContent(lastSaved(replyKey('session-unanswered')))).toEqual(before);

    unmount(pane);
  });

  it('undoes removals newest first, each quote going back to its own place', async () => {
    const { pane, before } = await removeMiddleQuote('session-two');
    const [first] = before.quotes;
    await removeQuote(first.id);
    expect(quoteCount()).toBe(1);

    await undoRemoval();
    expect(
      composerDraftFromContent(lastSaved(replyKey('session-two'))).quotes.map((q) => q.quote)
    ).toEqual(['First.', 'Third.']);
    await undoRemoval();
    expect(composerDraftFromContent(lastSaved(replyKey('session-two')))).toEqual(before);

    unmount(pane);
  });

  it('puts nothing back once the draft the quote came from has been sent', async () => {
    const { pane } = await removeMiddleQuote('session-sent');

    document.querySelector<HTMLElement>('[data-testid="composer-send"]')!.click();
    await settle();
    expect(quoteCount()).toBe(0);
    expect(writer.clearNow).toHaveBeenCalledWith(replyKey('session-sent'));

    // The draft the block came from has gone with the message, so an undo
    // that reaches the removal now restores nothing — neither into the empty
    // composer nor into the cleared stored draft.
    writer.scheduleSave.mockClear();
    await undoRemoval();
    expect(undoResults()).toEqual([false]);
    expect(quoteCount()).toBe(0);
    expect(lastSaved(replyKey('session-sent'))).toBeUndefined();
    expect(saveDraftMock).not.toHaveBeenCalled();

    unmount(pane);
  });

  it('still undoes the removal after a failed send, whose draft is put back', async () => {
    const { pane, before } = await removeMiddleQuote('session-failed');
    resumeSessionMock.mockRejectedValueOnce(new Error('agent gone'));

    document.querySelector<HTMLElement>('[data-testid="composer-send"]')!.click();
    await settle();
    // The draft is back in the box, the block it lost still restorable.
    expect(quoteCount()).toBe(2);

    await undoRemoval();
    expect(undoResults()).toEqual([true]);
    expect(quoteCount()).toBe(3);
    expect(composerDraftFromContent(lastSaved(replyKey('session-failed')))).toEqual(before);

    unmount(pane);
  });

  it('puts nothing back once the pane has turned to another session', async () => {
    const { pane, props } = await removeMiddleQuote('session-left');

    props.sessionId = 'session-next';
    flushSync();
    await settle();
    writer.scheduleSave.mockClear();

    await undoRemoval();
    expect(undoResults()).toEqual([false]);
    expect(quoteCount()).toBe(0);
    expect(lastSaved(replyKey('session-next'))).toBeUndefined();
    expect(saveDraftMock).not.toHaveBeenCalled();

    unmount(pane);
  });

  it('holds an undo that lands while the draft is reloading until the draft has landed', async () => {
    // The pane goes inactive (the note's chat closed) and comes back for the
    // same session. Its draft is loading again when the removal is undone:
    // the restore waits for the load, rather than being applied to the
    // composer and then replaced by the stored draft.
    const { pane, props, before, second } = await removeMiddleQuote('session-reload');
    const stored = lastSaved(replyKey('session-reload'));

    props.active = false;
    flushSync();
    await settle();

    let land!: (value: typeof stored) => void;
    loadDraftMock.mockImplementation(
      () =>
        new Promise<typeof stored>((resolve) => {
          land = resolve;
        })
    );
    props.active = true;
    flushSync();
    await settle();

    await undoRemoval();
    expect(undoResults()).toEqual([true]);
    expect(saveDraftMock).not.toHaveBeenCalled();
    // The composer hears the block is back only once it is: not yet, while
    // it is still held with the restore, or the caret would be sent to a
    // block that isn't rendered.
    expect(restoredQuotes()).toEqual([]);

    land(stored);
    await settle();
    expect(quoteCount()).toBe(3);
    expect(composerDraftFromContent(lastSaved(replyKey('session-reload')))).toEqual(before);
    expect(restoredQuotes()).toEqual([second.id]);

    unmount(pane);
  });

  it('puts nothing back, in the composer or the stored draft, while the pane shows no session', async () => {
    const { pane, props } = await removeMiddleQuote('session-away');
    const stored = lastSaved(replyKey('session-away'));

    props.active = false;
    flushSync();
    await settle();
    loadDraftMock.mockImplementation(async () => stored);
    writer.scheduleSave.mockClear();
    writer.run.mockClear();

    await undoRemoval();

    expect(undoResults()).toEqual([false]);
    expect(writer.run).not.toHaveBeenCalled();
    expect(saveDraftMock).not.toHaveBeenCalled();
    expect(lastSaved(replyKey('session-away'))).toBeUndefined();

    unmount(pane);
  });

  it('holds a quote that arrives during the draft load until the draft lands', async () => {
    let resolveLoad!: (value: string | null) => void;
    loadDraftMock.mockImplementation(
      () =>
        new Promise<string | null>((resolve) => {
          resolveLoad = resolve;
        })
    );
    const { pane } = mountPane('session-slow');
    flushSync();
    await settle();

    await addQuoteToReplyDraft('session-slow', quote);
    await settle();
    expect(quoteCount()).toBe(0);

    resolveLoad('stored text');
    await settle();

    expect(quoteCount()).toBe(1);
    const draft = composerDraftFromContent(lastSaved(replyKey('session-slow')));
    expect(draft.quotes[0]).toMatchObject({ before: 'stored text', reply: 'Which one?' });

    unmount(pane);
  });
});
