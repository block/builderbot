import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session, SessionMessage, SessionPollRequest } from '../../types';

/**
 * The bare "Done" a note session sends after a saved `write_note` call. The
 * pane leaves that message out of the transcript (see `noteDoneMessage.ts`);
 * any other text after the call is shown as written. These tests go through
 * the real pane, the real tool row and the real note card, so the rule is
 * checked where the user sees it, on rows shaped as the bridges store them.
 * A saved call shows as the note card and any other as its tool row (see
 * `SessionChatPane.noteCard.component.test.ts`).
 */

const SESSION_ID = 'note-session';

/** What the backend holds: the pane loads it, and polls it while `running`. */
const { transcript, sessionStatus, sessionRow, pollSessionMock } = vi.hoisted(() => {
  const transcript = { current: [] as SessionMessage[] };
  const sessionStatus = { current: 'completed' };
  const sessionRow = (id: string) =>
    ({ id, status: sessionStatus.current, provider: null, workingDir: null }) as unknown as Session;
  return {
    transcript,
    sessionStatus,
    sessionRow,
    // One tick of a running session: the last row the pane holds, which may
    // have grown, and any after it.
    pollSessionMock: vi.fn(async (id: string, request: SessionPollRequest) => ({
      session: sessionRow(id),
      queuedMessages: [],
      messages: transcript.current.filter((message) => message.id >= request.sinceId),
      acpMetadataMessages: [],
    })),
  };
});

// The composer and the pickers are not under test. The tool row and the note
// card are: neither is stubbed, so the saved call renders as the user sees it.
vi.mock('./ChatComposer.svelte', async () => {
  const { default: PromptInputStub } = await import('./testStubs/PromptInputStub.svelte');
  return { default: PromptInputStub, composerControlClass: '' };
});
vi.mock('../agents/AcpFixedConfigPicker.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./PlanCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./PipelineSteps.svelte', () => import('./testStubs/EmptyStub.svelte'));
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
vi.mock('../../shared/draftCache', () => ({
  createDraftWriteScheduler: () => ({
    scheduleSave: vi.fn(),
    saveNow: vi.fn(async () => {}),
    clearNow: vi.fn(async () => {}),
    flush: vi.fn(async () => {}),
  }),
  loadDraft: async () => null,
  loadDraftContent: async () => null,
}));
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  getSession: async (id: string) => sessionRow(id),
  pollSession: pollSessionMock,
  getSessionLiveDelivery: async () => ({ supported: false, retryable: false }),
  getLatestSessionMessages: async () => ({ data: transcript.current }),
  getSessionAcpMetadataMessages: async () => [],
  getSessionAcpMetadataMessagesSince: async () => [],
  listQueuedSessionMessages: async () => [],
}));

import SessionChatPane from './SessionChatPane.svelte';
import { FALLBACK_POLL_MS } from './messagePoll';
import { reactiveProps } from './testStubs/reactiveProps.svelte';

function row(id: number, role: SessionMessage['role'], content: string): SessionMessage {
  return { id, sessionId: SESSION_ID, role, createdAt: id, content };
}

const NOTE_TITLE = 'Plan: fix the flaky login test';

/** The call's arguments: the suggested next steps only. The title is in the draft file. */
const NOTE_INPUT: Record<string, unknown> = {
  suggested_next_steps: { suggestedNextCommitStep: 'Implement this plan' },
};
/** The arguments of a call recorded before the title moved into the draft file. */
const TITLED_NOTE_INPUT = { title: NOTE_TITLE, ...NOTE_INPUT };

/** A `write_note` call row as a bridge stores it, under that bridge's tool name. */
function noteCall(
  id: number,
  name: string,
  status = 'completed',
  input = NOTE_INPUT
): SessionMessage {
  return {
    ...row(id, 'tool_call', JSON.stringify({ name, input })),
    acpEventKind: 'tool_call',
    acpToolCallId: `tc-${id}`,
    acpToolStatus: status,
    acpRawInput: input,
  };
}

const CLAUDE = 'mcp__staged__write_note';
const GOOSE = 'staged: write note';
const CODEX = 'mcp.staged.write_note';

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

describe('SessionChatPane after a saved note', () => {
  let pane: ReturnType<typeof mount> | null = null;

  beforeEach(() => {
    document.body.innerHTML = '';
    sessionStatus.current = 'completed';
    pollSessionMock.mockClear();
    vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
    // A group that enters a mounted transcript slides in, and jsdom has no
    // Web Animations. The slide itself is beside the point here.
    Element.prototype.animate = () =>
      ({ onfinish: null, cancel() {}, playState: 'finished' }) as unknown as Animation;
  });

  afterEach(() => {
    if (pane) unmount(pane);
    pane = null;
    delete (Element.prototype as Partial<Element>).animate;
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  async function mountPane() {
    const props = reactiveProps({ active: true, sessionId: SESSION_ID, hashtagItems: [] });
    const target = document.createElement('div');
    document.body.appendChild(target);
    pane = mount(SessionChatPane, { target, props });
    await settle();
  }

  /** The text of each assistant message the transcript shows, in order. */
  const assistantTexts = () =>
    [...document.querySelectorAll('.assistant-message .markdown-content')].map((el) =>
      el.textContent?.trim()
    );

  /** The label of each note card the transcript shows, in order. */
  const noteCards = () =>
    [...document.querySelectorAll('.messages .note-activity-card')].map((card) =>
      card.textContent?.trim()
    );

  /** The header of each tool row: its verb and what it acted on. */
  const toolRows = () =>
    [...document.querySelectorAll('.tool-group .tool-header')].map((header) => [
      header.querySelector('.tool-name')?.textContent?.trim(),
      header.querySelector('.tool-args-preview')?.textContent?.trim(),
    ]);

  /** The keys of the groups the transcript rendered, in order. */
  const groupKeys = () =>
    [...document.querySelectorAll<HTMLElement>('.transcript-group')].map(
      (group) => group.dataset.groupKey
    );

  it.each([
    ['Claude Code', CLAUDE],
    ['Goose', GOOSE],
    ['Codex', CODEX],
  ])('leaves out the bare Done after a call from %s', async (_agent, name) => {
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      row(2, 'assistant', 'The test races the session cookie.'),
      noteCall(3, name),
      row(4, 'assistant', 'Done'),
    ];
    await mountPane();

    // The reply before the call and the saved call are there; the Done is not.
    expect(assistantTexts()).toEqual(['The test races the session cookie.']);
    expect(noteCards()).toEqual(['Note created']);
    expect(toolRows()).toEqual([]);
    // Nothing is rendered for it at all: no empty group to take a gap in the
    // column or to anchor the scroll position.
    expect(groupKeys()).toEqual(['m-1', 'm-2', 't-tool:tc-3']);
    expect(document.querySelector('.messages')?.textContent).not.toMatch(/done/i);
  });

  it("leaves it out beside Codex's extra item, and when it is said twice", async () => {
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CODEX),
      {
        ...row(3, 'tool_call', 'Guardian Review'),
        acpEventKind: 'tool_call',
        acpToolCallId: 'tc-review',
        acpToolStatus: 'completed',
      },
      row(4, 'assistant', 'Done'),
      row(5, 'assistant', '**Done.**'),
    ];
    await mountPane();

    expect(assistantTexts()).toEqual([]);
    expect(noteCards()).toEqual(['Note created']);
    expect(toolRows()).toEqual([['Ran', 'Guardian Review']]);
    expect(groupKeys()).toHaveLength(2);
  });

  it('shows any other text after the call, whole', async () => {
    transcript.current = [
      row(1, 'user', 'Which file holds the bug?'),
      noteCall(2, CLAUDE),
      row(3, 'assistant', 'It is `src/login.ts`, line 12.\n\nDone'),
      row(4, 'user', 'Add that to the note'),
      noteCall(5, CLAUDE),
      row(6, 'assistant', 'Done, the note now names the file.'),
    ];
    await mountPane();

    // An answer that ends with a Done line keeps the line; a sentence that
    // starts with the word keeps the word.
    expect(assistantTexts()).toEqual([
      'It is src/login.ts, line 12.\nDone',
      'Done, the note now names the file.',
    ]);
  });

  it('shows the Done after a rejected call', async () => {
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE, 'failed'),
      row(3, 'assistant', 'Done'),
    ];
    await mountPane();

    // The row is its verb alone: the call has no title.
    expect(toolRows()).toEqual([['Note rejected', undefined]]);
    expect(assistantTexts()).toEqual(['Done']);
  });

  it('treats calls recorded with a title as before', async () => {
    // A transcript from before the title moved into the draft file: the
    // rejected call's row is named after the title it passed, its Done
    // shows, and the Done after the saved call is left out.
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE, 'failed', TITLED_NOTE_INPUT),
      row(3, 'assistant', 'Done'),
      row(4, 'user', 'The diagram is broken. Fix it.'),
      noteCall(5, CLAUDE, 'completed', TITLED_NOTE_INPUT),
      row(6, 'assistant', 'Done'),
    ];
    await mountPane();

    expect(toolRows()).toEqual([['Note rejected', NOTE_TITLE]]);
    expect(assistantTexts()).toEqual(['Done']);
  });

  it('shows the Done after a call that was interrupted', async () => {
    // The call never settled and the session ended in error.
    sessionStatus.current = 'error';
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE, 'in_progress'),
      row(3, 'assistant', 'Done'),
    ];
    await mountPane();

    expect(toolRows()).toEqual([['Note not saved', undefined]]);
    expect(assistantTexts()).toEqual(['Done']);
  });

  it('leaves it out after a remote call completed from the outbox', async () => {
    // The call's update was lost with the connection; the session completed
    // from the workstation outbox, so the call resolves to saved.
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE, 'in_progress'),
      row(3, 'assistant', 'Done'),
    ];
    await mountPane();

    expect(noteCards()).toEqual(['Note created']);
    expect(toolRows()).toEqual([]);
    expect(assistantTexts()).toEqual([]);
  });

  it('brings the message back when more text streams in after the word', async () => {
    // Only the poll's interval: Svelte's own scheduling stays real. No session
    // stream is started here, so the pane is on its fallback poll.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    sessionStatus.current = 'running';
    transcript.current = [
      row(1, 'user', 'Which file holds the bug?'),
      noteCall(2, CLAUDE),
      row(3, 'assistant', 'Done'),
    ];
    await mountPane();
    expect(assistantTexts()).toEqual([]);
    expect(pollSessionMock).not.toHaveBeenCalled();

    // The next chunk: the same row, longer.
    transcript.current = [
      ...transcript.current.slice(0, 2),
      row(3, 'assistant', 'Done. The bug is in `src/login.ts`.'),
    ];
    vi.advanceTimersByTime(FALLBACK_POLL_MS);
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    await settle();

    expect(assistantTexts()).toEqual(['Done. The bug is in src/login.ts.']);
    expect(groupKeys()).toEqual(['m-1', 't-tool:tc-2', 'm-3']);
  });

  it('shows the message again once a tool call follows it', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    sessionStatus.current = 'running';
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE),
      row(3, 'assistant', 'Done'),
    ];
    await mountPane();
    expect(assistantTexts()).toEqual([]);

    transcript.current = [
      ...transcript.current,
      {
        ...row(4, 'tool_call', JSON.stringify({ name: 'Bash', input: { command: 'git status' } })),
        acpEventKind: 'tool_call',
        acpToolCallId: 'tc-4',
        acpToolStatus: 'in_progress',
      },
    ];
    vi.advanceTimersByTime(FALLBACK_POLL_MS);
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    await settle();

    expect(assistantTexts()).toEqual(['Done']);
  });
});
