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

  type UndoToast = { onClick: () => void };

  /** The Undo action of the newest "Quote removed" toast, with the toast's id. */
  function lastUndo(): { id: string; action: UndoToast } {
    const call = toastMock.mock.calls[toastMock.mock.calls.length - 1] as unknown as [
      string,
      { action: UndoToast },
    ];
    const id = toastMock.mock.results[toastMock.mock.results.length - 1].value as string;
    return { id, action: call[1].action };
  }

  /**
   * Mount a pane whose draft holds three quotes, then remove the middle one
   * (which has a reply, so Undo is offered). Returns the draft as saved
   * before the removal, for Undo to be measured against.
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
    document.querySelector<HTMLElement>(`[data-testid="remove-quote-${second.id}"]`)!.click();
    await settle();
    expect(quoteCount()).toBe(2);
    return { pane, props, before, second };
  }

  function quoteCount(): number {
    const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
    return Number(input?.dataset.quoteCount ?? NaN);
  }

  /** The quote replies the pane asked the composer to put the caret in. */
  function focusedQuotes(): string[] {
    const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
    return (input?.dataset.focusedQuotes ?? '').split(' ').filter(Boolean);
  }

  /** The quote blocks the pane asked the composer to scroll to without focusing. */
  function revealedQuotes(): string[] {
    const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
    return (input?.dataset.revealedQuotes ?? '').split(' ').filter(Boolean);
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

  it('offers to undo removing a quote with a reply, putting it back in its place', async () => {
    toastMock.mockClear();
    const { pane } = mountPane('session-undo');
    flushSync();
    await settle();

    await addQuoteToReplyDraft('session-undo', { ...quote, quote: 'First.' });
    await addQuoteToReplyDraft('session-undo', { ...quote, quote: 'Second.' });
    await addQuoteToReplyDraft('session-undo', { ...quote, quote: 'Third.', reply: '' });
    await settle();
    const before = composerDraftFromContent(lastSaved(replyKey('session-undo')));
    const [, second] = before.quotes;

    document.querySelector<HTMLElement>(`[data-testid="remove-quote-${second.id}"]`)!.click();
    await settle();

    expect(quoteCount()).toBe(2);
    expect(toastMock).toHaveBeenCalledTimes(1);
    const [, options] = toastMock.mock.calls[0] as [string, { action: { onClick: () => void } }];
    expect(options.action).toMatchObject({ label: 'Undo' });

    options.action.onClick();
    await settle();

    expect(quoteCount()).toBe(3);
    const restored = composerDraftFromContent(lastSaved(replyKey('session-undo')));
    expect(restored.quotes.map((q) => q.quote)).toEqual(['First.', 'Second.', 'Third.']);
    expect(restored.quotes[1]).toMatchObject({ id: second.id, reply: 'Which one?' });
    expect(restored).toEqual(before);

    // An unanswered quote just goes.
    const third = restored.quotes[2];
    document.querySelector<HTMLElement>(`[data-testid="remove-quote-${third.id}"]`)!.click();
    await settle();
    expect(quoteCount()).toBe(2);
    expect(toastMock).toHaveBeenCalledTimes(1);

    unmount(pane);
  });

  it('puts the caret in the restored reply for a pointer, and only scrolls to the block on a touch screen', async () => {
    const first = await removeMiddleQuote('session-caret');
    lastUndo().action.onClick();
    await settle();
    expect(quoteCount()).toBe(3);
    // The focus brings the block into the field's view itself.
    expect(focusedQuotes()).toEqual([first.second.id]);
    expect(revealedQuotes()).toEqual([]);
    unmount(first.pane);

    // A touch screen's keyboard would rise over the restored draft, so the
    // field is scrolled to the block and nothing is focused.
    viewportMock.hasCoarsePointer = true;
    try {
      const second = await removeMiddleQuote('session-caret-touch');
      lastUndo().action.onClick();
      await settle();
      expect(quoteCount()).toBe(3);
      expect(focusedQuotes()).toEqual([]);
      expect(revealedQuotes()).toEqual([second.second.id]);
      unmount(second.pane);
    } finally {
      viewportMock.hasCoarsePointer = false;
    }
  });

  it('takes the Undo toast down with a send, and a late Undo puts nothing back', async () => {
    const { pane } = await removeMiddleQuote('session-sent');
    const undo = lastUndo();

    document.querySelector<HTMLElement>('[data-testid="composer-send"]')!.click();
    await settle();
    expect(quoteCount()).toBe(0);
    expect(writer.clearNow).toHaveBeenCalledWith(replyKey('session-sent'));
    expect(toastMock.dismiss).toHaveBeenCalledWith(undo.id);

    // The toast's leaving animation can still take a click: the draft the
    // block came from has gone with the message, so nothing is restored —
    // neither into the empty composer nor into the cleared stored draft.
    writer.scheduleSave.mockClear();
    undo.action.onClick();
    await settle();
    expect(quoteCount()).toBe(0);
    expect(lastSaved(replyKey('session-sent'))).toBeUndefined();
    expect(saveDraftMock).not.toHaveBeenCalled();

    unmount(pane);
  });

  it('keeps the Undo toast through a failed send, whose draft is put back', async () => {
    const { pane, before } = await removeMiddleQuote('session-failed');
    const undo = lastUndo();
    resumeSessionMock.mockRejectedValueOnce(new Error('agent gone'));

    document.querySelector<HTMLElement>('[data-testid="composer-send"]')!.click();
    await settle();
    // The draft is back in the box, the block it lost still restorable.
    expect(quoteCount()).toBe(2);
    expect(toastMock.dismiss).not.toHaveBeenCalled();

    undo.action.onClick();
    await settle();
    expect(quoteCount()).toBe(3);
    expect(composerDraftFromContent(lastSaved(replyKey('session-failed')))).toEqual(before);

    // Pressed, the toast is gone with the press; the send that follows has
    // nothing left to take down.
    document.querySelector<HTMLElement>('[data-testid="composer-send"]')!.click();
    await settle();
    expect(quoteCount()).toBe(0);
    expect(toastMock.dismiss).not.toHaveBeenCalled();

    unmount(pane);
  });

  it('takes the Undo toast down when the pane turns to another session', async () => {
    const { pane, props } = await removeMiddleQuote('session-left');
    const undo = lastUndo();

    props.sessionId = 'session-next';
    flushSync();
    await settle();
    expect(toastMock.dismiss).toHaveBeenCalledWith(undo.id);

    unmount(pane);
  });

  it('holds an Undo that lands while the draft is reloading until the draft has landed', async () => {
    // The pane goes inactive (the note's chat closed) and comes back for the
    // same session within the toast's lifetime. Its draft is loading again
    // when Undo is pressed: the restore waits for the load, rather than
    // being applied to the composer and then replaced by the stored draft.
    const { pane, props, before, second } = await removeMiddleQuote('session-reload');
    const stored = lastSaved(replyKey('session-reload'));
    const undo = lastUndo();

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
    expect(toastMock.dismiss).not.toHaveBeenCalled();

    undo.action.onClick();
    await settle();
    expect(saveDraftMock).not.toHaveBeenCalled();
    // The caret follows the block into the composer: not yet, while the
    // block is still held with the restore, or it would be looked for
    // before it is rendered and the ask dropped.
    expect(focusedQuotes()).toEqual([]);
    expect(revealedQuotes()).toEqual([]);

    land(stored);
    await settle();
    expect(quoteCount()).toBe(3);
    expect(composerDraftFromContent(lastSaved(replyKey('session-reload')))).toEqual(before);
    expect(focusedQuotes()).toEqual([second.id]);
    expect(revealedQuotes()).toEqual([]);

    unmount(pane);
  });

  it('restores into the stored draft when the pane no longer shows the session', async () => {
    const { pane, props, before } = await removeMiddleQuote('session-away');
    const stored = lastSaved(replyKey('session-away'));
    const undo = lastUndo();

    props.active = false;
    flushSync();
    await settle();
    loadDraftMock.mockImplementation(async () => stored);

    undo.action.onClick();
    await settle();

    // Through the shared writer, so it queues behind any pending save.
    expect(writer.run).toHaveBeenCalledTimes(1);
    expect(saveDraftMock).toHaveBeenCalledTimes(1);
    const [key, content] = saveDraftMock.mock.calls[0];
    expect(key).toBe(replyKey('session-away'));
    expect(
      composerDraftFromContent(content as Parameters<typeof composerDraftFromContent>[0])
    ).toEqual(before);

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
