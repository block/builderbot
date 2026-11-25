import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session, SessionMessage, SessionPollRequest } from '../../types';
import type { LinkedNoteContext } from './noteFreshness';

/**
 * The note card a note session's chat shows where a note was delivered:
 * "Note created" for the first delivery, "Note updated" for later ones. A
 * note delivered with the `write_note` tool shows the card in place of the
 * call's "Saved note" row (see `noteDeliveries.ts`).
 *
 * These tests mount the real pane with the real card and the real tool row,
 * on rows shaped as the bridges store them. The card went missing once
 * because every pane test stubbed it and the helper tests used fixtures in
 * the old text shape, so neither is replaced here.
 */

const SESSION_ID = 'note-session';

/** What the backend holds: the pane loads it, and polls it while `running`. */
const { transcript, sessionState, sessionRow, pollSessionMock } = vi.hoisted(() => {
  const transcript = { current: [] as SessionMessage[] };
  const sessionState = { status: 'completed', completionReason: 'turn_complete' as string | null };
  const sessionRow = (id: string) =>
    ({
      id,
      status: sessionState.status,
      completionReason: sessionState.completionReason,
      provider: null,
      workingDir: null,
    }) as unknown as Session;
  return {
    transcript,
    sessionState,
    sessionRow,
    // One tick of a running session: the last row the pane holds, which may
    // have changed, and any after it.
    pollSessionMock: vi.fn(async (id: string, request: SessionPollRequest) => ({
      session: sessionRow(id),
      queuedMessages: [],
      messages: transcript.current.filter((message) => message.id >= request.sinceId),
      acpMetadataMessages: [],
    })),
  };
});

// The composer and the pickers are not under test. `NoteActivityCard` and
// `ToolCallCard` are, and are the real components.
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

/** A row created at the time its id names, so ids read as a clock. */
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

/** The note the pane is opened on, as the note dialog hands it over. */
function noteContext(updatedAt: number): LinkedNoteContext {
  return {
    id: 'note-1',
    title: NOTE_TITLE,
    content: 'Wait for the session cookie before asserting.',
    updatedAt,
    hasParsedNote: true,
  };
}

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

describe('SessionChatPane note card', () => {
  let pane: ReturnType<typeof mount> | null = null;
  /** The pane's props, reactive so a test can hand it a refreshed note. */
  let props: {
    active: boolean;
    sessionId: string;
    hashtagItems: [];
    noteInfo: LinkedNoteContext | null;
  };

  beforeEach(() => {
    document.body.innerHTML = '';
    sessionState.status = 'completed';
    sessionState.completionReason = 'turn_complete';
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

  /** Mounts the pane, on the note when one is given, as the note dialog does. */
  async function mountPane(noteInfo: LinkedNoteContext | null = null) {
    props = reactiveProps({ active: true, sessionId: SESSION_ID, hashtagItems: [], noteInfo });
    const target = document.createElement('div');
    document.body.appendChild(target);
    pane = mount(SessionChatPane, { target, props });
    await settle();
  }

  /** One poll of a running session, which picks up the rows and the status. */
  async function pollOnce() {
    const calls = pollSessionMock.mock.calls.length;
    vi.advanceTimersByTime(FALLBACK_POLL_MS);
    expect(pollSessionMock).toHaveBeenCalledTimes(calls + 1);
    await settle();
  }

  /** The label of each note card the transcript shows, in order. */
  const noteCards = () =>
    [...document.querySelectorAll('.messages .note-activity-card')].map((card) =>
      card.textContent?.trim()
    );

  /**
   * The header of each tool row: its verb and what it acted on. A `write_note`
   * row has only the verb, unless the call was recorded with a title.
   */
  const toolRows = () =>
    [...document.querySelectorAll('.tool-group .tool-header')].map((header) => [
      header.querySelector('.tool-name')?.textContent?.trim(),
      header.querySelector('.tool-args-preview')?.textContent?.trim(),
    ]);

  /** The label of the follow-up button under the transcript, if it is shown. */
  const followupButton = () =>
    document.querySelector('.note-followup-row button')?.textContent?.trim() ?? null;

  /** What the transcript shows, top to bottom: replies, tool rows and cards. */
  const shown = () =>
    [
      ...document.querySelectorAll(
        '.messages .human-text, .messages .markdown-content, .messages .tool-name, .messages .note-activity-card'
      ),
    ].map((el) => el.textContent?.trim());

  it.each([
    ['Claude Code', CLAUDE],
    ['Goose', GOOSE],
    ['Codex', CODEX],
  ])('shows the card in place of a saved call from %s', async (_agent, name) => {
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      row(2, 'assistant', 'The test races the session cookie.'),
      noteCall(3, name),
      row(4, 'assistant', 'Done'),
    ];
    await mountPane();

    expect(noteCards()).toEqual(['Note created']);
    // The call has no row of its own.
    expect(toolRows()).toEqual([]);
    // The card is where the row was: in the call's tool group, after the
    // reply, and it ends the turn since the closing "Done" is hidden.
    expect(document.querySelectorAll('.tool-group > .note-activity-card')).toHaveLength(1);
    expect(shown()).toEqual([
      'Plan a fix for the flaky login test',
      'The test races the session cookie.',
      'Note created',
    ]);
  });

  it('labels the first saved call as created and later ones as updated', async () => {
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE),
      row(3, 'assistant', 'Done'),
      row(4, 'user', 'Add the file name'),
      noteCall(5, CLAUDE),
      row(6, 'assistant', 'Done'),
    ];
    await mountPane();

    expect(shown()).toEqual([
      'Plan a fix for the flaky login test',
      'Note created',
      'Add the file name',
      'Note updated',
    ]);
  });

  it('shows a reply that follows the call under the card', async () => {
    transcript.current = [
      row(1, 'user', 'Which file holds the bug? Add it to the note.'),
      noteCall(2, CLAUDE),
      row(3, 'assistant', 'It is `src/login.ts`, line 12.'),
    ];
    await mountPane();

    expect(shown()).toEqual([
      'Which file holds the bug? Add it to the note.',
      'Note created',
      'It is src/login.ts, line 12.',
    ]);
  });

  it('keeps the tool row of a rejected call', async () => {
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE, 'failed'),
      row(3, 'assistant', 'The diagram did not render. Fixing it.'),
      noteCall(4, CLAUDE),
      row(5, 'assistant', 'Done'),
    ];
    await mountPane();

    // The rejected call stays a row, the verb alone; the retry is the card,
    // and the first delivery, since the rejected call delivered nothing.
    expect(toolRows()).toEqual([['Note rejected', undefined]]);
    expect(shown()).toEqual([
      'Plan a fix for the flaky login test',
      'Note rejected',
      'The diagram did not render. Fixing it.',
      'Note created',
    ]);
  });

  it('renders calls recorded with a title as before', async () => {
    // A transcript from before the title moved into the draft file: each
    // call passed the title, and a call that kept its row is named after it.
    const titled = (id: number, status?: string) => noteCall(id, CLAUDE, status, TITLED_NOTE_INPUT);
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      titled(2, 'failed'),
      row(3, 'assistant', 'The diagram did not render. Fixing it.'),
      titled(4),
      row(5, 'assistant', 'Done'),
    ];
    await mountPane();

    expect(toolRows()).toEqual([['Note rejected', NOTE_TITLE]]);
    expect(noteCards()).toEqual(['Note created']);
    expect(shown()).toEqual([
      'Plan a fix for the flaky login test',
      'Note rejected',
      'The diagram did not render. Fixing it.',
      'Note created',
    ]);
    // The saved call is the card, with no row of its own and no title beside
    // it: the rejected call's row is the only place the title shows.
    expect(document.querySelector('.messages')?.textContent?.split(NOTE_TITLE)).toHaveLength(2);
  });

  it('keeps the tool row of an interrupted call', async () => {
    // The call never settled and the session ended in error.
    sessionState.status = 'error';
    sessionState.completionReason = 'crashed';
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE, 'in_progress'),
    ];
    await mountPane();

    expect(toolRows()).toEqual([['Note not saved', undefined]]);
    expect(noteCards()).toEqual([]);
  });

  it('shows the card for a remote call completed from the outbox', async () => {
    // The call's update was lost with the connection; the session completed
    // from the workstation outbox, so the call resolves to saved.
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE, 'in_progress'),
    ];
    await mountPane();

    expect(toolRows()).toEqual([]);
    expect(noteCards()).toEqual(['Note created']);
  });

  it("keeps Codex's extra item as a row beside the card", async () => {
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
    ];
    await mountPane();

    expect(noteCards()).toEqual(['Note created']);
    expect(toolRows()).toHaveLength(1);
    // Both are in the one tool group, the card first.
    const group = document.querySelector('.tool-group');
    expect(group?.children).toHaveLength(2);
    expect(group?.firstElementChild?.classList.contains('note-activity-card')).toBe(true);
  });

  it('shows two saved calls in a row as two cards', async () => {
    // Same verb, same tool group: as rows they would fold into one collapsed
    // "Saved note" row.
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE),
      noteCall(3, CLAUDE),
      row(4, 'assistant', 'Done'),
    ];
    await mountPane();

    expect(noteCards()).toEqual(['Note created', 'Note updated']);
    expect(toolRows()).toEqual([]);
  });

  it('turns the running call from a row into the card when it completes', async () => {
    // Only the poll's interval: Svelte's own scheduling stays real. No session
    // stream is started here, so the pane is on its fallback poll.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    sessionState.status = 'running';
    sessionState.completionReason = null;
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      noteCall(2, CLAUDE, 'in_progress'),
    ];
    await mountPane();
    expect(toolRows()).toEqual([['Saving note', undefined]]);
    expect(noteCards()).toEqual([]);

    transcript.current = [transcript.current[0], noteCall(2, CLAUDE)];
    await pollOnce();

    expect(toolRows()).toEqual([]);
    expect(noteCards()).toEqual(['Note created']);
  });

  it('still shows the card for a note delivered as text', async () => {
    // The fallback for a run with no tool, and every session recorded before
    // the tool: the reply holds the separator and the note after it.
    transcript.current = [
      row(1, 'user', 'Plan a fix for the flaky login test'),
      row(2, 'assistant', 'Here is the plan.\n\n---\n\n# Plan\n\nWait for the cookie.'),
      row(3, 'user', 'Add the file name'),
      noteCall(4, CLAUDE),
      row(5, 'assistant', 'Done'),
    ];
    await mountPane();

    // The text after the separator is the note, not chat; the saved call
    // that follows it is an update to the note that text created.
    expect(shown()).toEqual([
      'Plan a fix for the flaky login test',
      'Here is the plan.',
      'Note created',
      'Add the file name',
      'Note updated',
    ]);
  });

  describe('opened on its note', () => {
    const savedTurn = () => [
      row(10, 'user', 'Plan a fix for the flaky login test'),
      noteCall(20, CLAUDE),
      row(30, 'assistant', 'Done'),
    ];

    it('shows the card and no button when the turn changed the note', async () => {
      transcript.current = savedTurn();
      // Stored once the turn ended, after its last message.
      await mountPane(noteContext(40));

      expect(noteCards()).toEqual(['Note created']);
      expect(toolRows()).toEqual([]);
      expect(followupButton()).toBeNull();
    });

    it('keeps the row and offers the button when the note is older than the call', async () => {
      // The agent republished an identical note, which keeps its timestamp,
      // or the run failed before the note was stored.
      transcript.current = savedTurn();
      await mountPane(noteContext(5));

      expect(noteCards()).toEqual([]);
      expect(toolRows()).toEqual([['Saved note', undefined]]);
      expect(followupButton()).toBe('Ask for the note to be updated');
      // The closing "Done" stays hidden either way.
      expect(shown()).toEqual(['Plan a fix for the flaky login test', 'Saved note']);
    });

    it('swaps the row for the card when the refreshed note is newer', async () => {
      transcript.current = savedTurn();
      await mountPane(noteContext(5));
      expect(toolRows()).toEqual([['Saved note', undefined]]);

      // The dialog refetched the note: it was stored after the turn.
      props.noteInfo = noteContext(40);
      await settle();

      expect(noteCards()).toEqual(['Note created']);
      expect(toolRows()).toEqual([]);
      expect(followupButton()).toBeNull();
    });

    it('shows the card while the session runs, before the note is stored', async () => {
      vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
      sessionState.status = 'running';
      sessionState.completionReason = null;
      transcript.current = savedTurn();
      // The stored note is still the one from before the turn.
      await mountPane(noteContext(5));

      expect(noteCards()).toEqual(['Note created']);
      expect(toolRows()).toEqual([]);
      expect(followupButton()).toBeNull();

      // The session settles and the note did not change: the row comes back
      // and the button is offered.
      sessionState.status = 'completed';
      sessionState.completionReason = 'turn_complete';
      await pollOnce();

      expect(noteCards()).toEqual([]);
      expect(toolRows()).toEqual([['Saved note', undefined]]);
      expect(followupButton()).toBe('Ask for the note to be updated');
    });

    it('keeps an earlier card and offers the button after a chat-only turn', async () => {
      transcript.current = [
        ...savedTurn(),
        row(50, 'user', 'Why wait for the cookie?'),
        row(60, 'assistant', 'The assertion reads it before the redirect sets it.'),
      ];
      // Stored after the first turn; the second turn left it alone.
      await mountPane(noteContext(40));

      expect(shown()).toEqual([
        'Plan a fix for the flaky login test',
        'Note created',
        'Why wait for the cookie?',
        'The assertion reads it before the redirect sets it.',
      ]);
      expect(followupButton()).toBe('Ask for the note to be updated');
    });

    it('compares only the final turn', async () => {
      // The first turn stored the note at 40. The second republished it
      // unchanged: its call keeps the row, the first turn's stays a card.
      transcript.current = [
        ...savedTurn(),
        row(50, 'user', 'Is the plan still right?'),
        noteCall(60, CLAUDE),
        row(70, 'assistant', 'Done'),
      ];
      await mountPane(noteContext(40));

      expect(shown()).toEqual([
        'Plan a fix for the flaky login test',
        'Note created',
        'Is the plan still right?',
        'Saved note',
      ]);
      expect(followupButton()).toBe('Ask for the note to be updated');
    });
  });
});
