import { flushSync, mount, tick, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session } from '../../types';

const { writer, loadDraftMock } = vi.hoisted(() => ({
  writer: {
    scheduleSave: vi.fn(),
    saveNow: vi.fn(async () => {}),
    clearNow: vi.fn(async () => {}),
    flush: vi.fn(async () => {}),
  },
  loadDraftMock: vi.fn(async (_key: string): Promise<string | null> => null),
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
    const value = await loadDraftMock(key);
    return value === null ? null : { value };
  },
}));
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  getSession: async (id: string) =>
    ({ id, status: 'completed', provider: null, workingDir: null }) as unknown as Session,
  getSessionMessages: async () => ({ data: [] }),
  getSessionMessagesSince: async () => [],
  getSessionAcpMetadataMessages: async () => [],
  getSessionAcpMetadataMessagesSince: async () => [],
  listQueuedSessionMessages: async () => [],
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
    loadDraftMock.mockReset();
    loadDraftMock.mockImplementation(async () => null);
  });

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
