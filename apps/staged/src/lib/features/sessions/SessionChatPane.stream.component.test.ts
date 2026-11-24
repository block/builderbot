import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';
import type { SessionStreamHandlers } from '../../transport';
import type {
  Session,
  SessionFull,
  SessionMessage,
  SessionPollRequest,
  SessionStatus,
  SessionStreamInterest,
} from '../../types';

/**
 * The chat pane fed by the session stream: frames in place of the 500 ms
 * tick while the stream is established, and the tick whenever it is not.
 *
 * The owner is the real one, on a link the test plays the backend of, so a
 * frame takes the route it takes in the app: link, owner, pane.
 */

const SESSION_ID = 'streamed-session';

const { sessionRow, transcript, eventListeners, pollSessionMock, link } = vi.hoisted(() => {
  const sessionRow = { current: null as Session | null };
  const transcript = { current: [] as SessionMessage[] };
  return {
    sessionRow,
    transcript,
    eventListeners: new Map<string, Set<(payload: unknown) => void>>(),
    pollSessionMock: vi.fn(async (_id: string, request: SessionPollRequest) => ({
      session: sessionRow.current,
      queuedMessages: [],
      messages: transcript.current.filter((m) => m.id >= request.sinceId),
      acpMetadataMessages: [],
    })),
    /** The stream's link, as the backend sees it. */
    link: {
      handlers: null as SessionStreamHandlers | null,
      interests: [] as SessionStreamInterest[],
      generation: -1,
      seq: 0,
    },
  };
});

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
  listenToEvent: (event: string, callback: (payload: unknown) => void) => {
    let listeners = eventListeners.get(event);
    if (!listeners) eventListeners.set(event, (listeners = new Set()));
    listeners.add(callback);
    return () => listeners.delete(callback);
  },
}));
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  getSession: async () => sessionRow.current,
  pollSession: pollSessionMock,
  getSessionLiveDelivery: async () => ({ supported: false, retryable: false }),
  getLatestSessionMessages: async () => ({ data: transcript.current }),
  getSessionMessages: async () => ({ data: [] }),
  getSessionMessagesSince: async () => [],
  getSessionAcpMetadataMessages: async () => [],
  getSessionAcpMetadataMessagesSince: async () => [],
  listQueuedSessionMessages: async () => [],
}));
vi.mock('../../stores/sessionStream.svelte', async (importOriginal) => {
  const original = await importOriginal<typeof import('../../stores/sessionStream.svelte')>();
  const sessionStream = new original.SessionStreamOwner({
    open: (handlers) => {
      link.handlers = handlers;
      return {
        setInterest: async (interest) => {
          link.interests.push(interest);
        },
        check: async () => true,
        close: () => {},
      };
    },
    resolveRoots: async () => [],
    onStoreReset: () => () => {},
    onHealthTrigger: () => () => {},
    visibility: null,
  });
  return { ...original, sessionStream };
});

import SessionChatPane from './SessionChatPane.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';
import { STATUS_HOLD_MS } from './statusHold';
import { TRANSCRIPT_PAGE_ROWS } from './transcriptWindow';
import { INTEREST_DELAY_MS, sessionStream } from '../../stores/sessionStream.svelte';

function session(status: SessionStatus, overrides: Partial<Session> = {}): Session {
  return {
    id: SESSION_ID,
    status,
    provider: null,
    workingDir: null,
    errorMessage: null,
    completionReason: null,
    updatedAt: 1,
    ...overrides,
  } as unknown as Session;
}

function row(id: number, role: SessionMessage['role'], content: string): SessionMessage {
  return { id, sessionId: SESSION_ID, role, content, createdAt: id };
}

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

/** Let the owner's interest timer fire and its call settle. */
async function declared() {
  await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);
  await settle();
}

/** Start the window's stream and open its link, as the app does at startup. */
async function establish() {
  sessionStream.start();
  link.handlers?.onOpen();
  await declared();
  expect(sessionStream.availability).toBe('established');
}

function emit(event: string, payload: unknown) {
  for (const listener of [...(eventListeners.get(event) ?? [])]) listener(payload);
  flushSync();
}

const lastInterest = () => link.interests[link.interests.length - 1];
/** The pane's session in the interest last declared, if it is there. */
const chat = () => lastInterest()?.sessions.find((s) => s.level === 'full');

/** Deliver a full-level frame for the pane's session, as the backend's next frame. */
async function frame(kind: 'snapshot' | 'delta', full: Partial<SessionFull> = {}) {
  const generation = lastInterest().generation;
  if (link.generation !== generation) link.seq = 0;
  link.generation = generation;
  link.seq += 1;
  link.handlers?.onFrame({
    generation,
    seq: link.seq,
    sessionId: SESSION_ID,
    kind,
    full: { messages: [], acpMetadataMessages: [], ...full },
  });
  await settle();
}

const text = () => document.body.textContent ?? '';
const thinking = () => document.querySelector('.thinking') !== null;
const rowCount = (root: ParentNode = document) => root.querySelectorAll('.message-row').length;

describe('SessionChatPane on the session stream', () => {
  const panes: ReturnType<typeof mount>[] = [];
  let intervals: MockInstance<typeof setInterval>;
  /** How many times the poll's interval has been started. */
  const ticking = () => intervals.mock.calls.filter(([, ms]) => ms === 500).length;

  beforeEach(() => {
    document.body.innerHTML = '';
    eventListeners.clear();
    pollSessionMock.mockClear();
    link.interests.length = 0;
    link.seq = 0;
    link.generation = -1;
    sessionRow.current = session('running');
    transcript.current = [row(1, 'user', 'Explain the cache'), row(2, 'assistant', 'The cache')];
    vi.useFakeTimers({
      toFake: ['setInterval', 'clearInterval', 'setTimeout', 'clearTimeout', 'Date'],
    });
    intervals = vi.spyOn(globalThis, 'setInterval');
    vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
    Element.prototype.animate = () =>
      ({ onfinish: null, cancel() {}, playState: 'finished' }) as unknown as Animation;
  });

  afterEach(() => {
    for (const pane of panes.splice(0)) unmount(pane);
    sessionStream.stop();
    delete (Element.prototype as Partial<Element>).animate;
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  async function mountPane() {
    const props = reactiveProps({ active: true, sessionId: SESSION_ID, hashtagItems: [] });
    const target = document.createElement('div');
    document.body.appendChild(target);
    panes.push(mount(SessionChatPane, { target, props }));
    await settle();
    await declared();
    return { props, target, pane: panes[panes.length - 1] as { park(): void; unpark(): void } };
  }

  /** Whether `ms` of the poll's interval pass with no tick. */
  async function quietFor(ms: number) {
    const calls = pollSessionMock.mock.calls.length;
    await vi.advanceTimersByTimeAsync(ms);
    await settle();
    return pollSessionMock.mock.calls.length === calls;
  }

  it('asks the stream for a running session from what it loaded, and does not tick', async () => {
    await establish();
    await mountPane();

    expect(chat()).toEqual({
      sessionId: SESSION_ID,
      level: 'full',
      cursor: { sinceId: 2, refetchIds: [], metadataSinceId: 0, metadataRefetchIds: [] },
    });
    expect(await quietFor(5_000)).toBe(true);
    expect(pollSessionMock).not.toHaveBeenCalled();
    // Not an interval whose ticks are refused, either.
    expect(ticking()).toBe(0);
  });

  it('follows a session that is not running as well, so its next turn arrives', async () => {
    sessionRow.current = session('completed');
    await establish();
    await mountPane();
    expect(chat()?.cursor?.sinceId).toBe(2);
    expect(thinking()).toBe(false);

    // Started from elsewhere: no status event has to reach this pane.
    await frame('delta', {
      session: session('running', { updatedAt: 2 }),
      messages: [row(3, 'user', 'And the eviction?')],
    });
    expect(thinking()).toBe(true);
    expect(text()).toContain('And the eviction?');
    expect(pollSessionMock).not.toHaveBeenCalled();
  });

  it('merges a snapshot and the deltas after it by id', async () => {
    await establish();
    await mountPane();

    // Read from the pane's cursor, so it repeats the tail.
    await frame('snapshot', {
      session: session('running'),
      queuedMessages: [],
      messages: [row(2, 'assistant', 'The cache is')],
    });
    expect(text()).toContain('The cache is');

    // The text grows behind a tool call, and a row follows: neither is the
    // tail the pane holds, and the tail is not repeated.
    await frame('delta', { messages: [row(3, 'tool_call', 'Read')] });
    await frame('delta', {
      messages: [row(2, 'assistant', 'The cache is two maps'), row(4, 'assistant', 'Both are LRU')],
    });
    expect(text()).toContain('The cache is two maps');
    expect(text()).toContain('Both are LRU');
    expect(text()).toContain('Explain the cache');
    expect(pollSessionMock).not.toHaveBeenCalled();
  });

  it('keeps a status event’s status over an older frame, and fetches nothing for the final text', async () => {
    await establish();
    await mountPane();
    await frame('snapshot', { session: session('running'), queuedMessages: [] });
    expect(thinking()).toBe(true);

    emit('session-status-changed', { sessionId: SESSION_ID, status: 'completed' });
    await settle();
    expect(thinking()).toBe(false);

    // Read before the status was written, delivered after its event.
    await frame('delta', {
      session: session('running', { updatedAt: 2 }),
      messages: [row(2, 'assistant', 'The cache is two')],
    });
    expect(thinking()).toBe(false);
    expect(text()).toContain('The cache is two');

    // The frame that agrees comes last, with the turn's final text.
    await frame('delta', {
      session: session('completed', { updatedAt: 3 }),
      messages: [row(2, 'assistant', 'The cache is two maps.')],
    });
    expect(thinking()).toBe(false);
    expect(text()).toContain('The cache is two maps.');

    // The next turn's frame is believed again.
    await frame('delta', { session: session('running', { updatedAt: 4 }) });
    expect(thinking()).toBe(true);
    // The terminal status event's extra poll is the stream's to make up for.
    expect(pollSessionMock).not.toHaveBeenCalled();
    expect(ticking()).toBe(0);
  });

  it('takes a status no event announced from a frame', async () => {
    await establish();
    await mountPane();
    expect(thinking()).toBe(true);

    // Another Staged process stopped the session: its event is emitted there.
    await frame('delta', { session: session('cancelled', { updatedAt: 2 }) });
    expect(thinking()).toBe(false);
  });

  it('asks to be read again when no frame agrees with a status event in time', async () => {
    await establish();
    await mountPane();
    await frame('snapshot', { session: session('running'), queuedMessages: [] });
    const calls = link.interests.length;

    emit('session-status-changed', { sessionId: SESSION_ID, status: 'completed' });
    await vi.advanceTimersByTimeAsync(STATUS_HOLD_MS - 1);
    expect(link.interests).toHaveLength(calls);
    await vi.advanceTimersByTimeAsync(1);
    await declared();
    expect(link.interests).toHaveLength(calls + 1);
    expect(chat()?.cursor).toBeDefined();

    // Whatever the row says now is taken.
    await frame('snapshot', { session: session('running', { updatedAt: 9 }), queuedMessages: [] });
    expect(thinking()).toBe(true);
  });

  it('does not ask again once a frame has agreed', async () => {
    await establish();
    await mountPane();
    emit('session-status-changed', { sessionId: SESSION_ID, status: 'completed' });
    await frame('delta', { session: session('completed', { updatedAt: 2 }) });
    const calls = link.interests.length;

    await vi.advanceTimersByTimeAsync(STATUS_HOLD_MS * 2);
    expect(link.interests).toHaveLength(calls);
  });

  it('shows the queue a frame brings', async () => {
    await establish();
    await mountPane();
    await frame('delta', {
      queuedMessages: [
        {
          id: 'q1',
          sessionId: SESSION_ID,
          branchId: null,
          content: 'Then tidy the tests',
          imageIds: [],
          status: 'queued',
          lastError: null,
          createdAt: 5,
          updatedAt: 5,
          claimedAt: null,
          ownerPid: null,
          sentMessageId: null,
        },
      ],
    });
    expect(text()).toContain('Then tidy the tests');
  });

  it('polls as it always did while the stream is not established', async () => {
    // The kill switch is off, or the stream could not be opened.
    expect(sessionStream.availability).toBe('unavailable');
    await mountPane();
    expect(link.interests).toHaveLength(0);

    transcript.current = [...transcript.current, row(3, 'assistant', 'It is two maps')];
    expect(await quietFor(500)).toBe(false);
    expect(text()).toContain('It is two maps');
    expect(await quietFor(500)).toBe(false);
    expect(ticking()).toBe(1);
  });

  it('goes back to the poll when the stream is lost, and to the stream when it returns', async () => {
    await establish();
    await mountPane();
    expect(pollSessionMock).not.toHaveBeenCalled();

    // The socket drops, and with it whatever had not been delivered.
    transcript.current = [...transcript.current, row(3, 'assistant', 'It is two maps')];
    link.handlers?.onClose();
    await settle();
    // One read from the cursor at once, then the tick.
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    expect(pollSessionMock.mock.calls[0][1].sinceId).toBe(2);
    expect(text()).toContain('It is two maps');
    expect(await quietFor(500)).toBe(false);

    link.handlers?.onOpen();
    await declared();
    expect(sessionStream.availability).toBe('established');
    // The pane asks once the new generation is established, for a snapshot
    // from what it holds now.
    await declared();
    expect(chat()?.cursor?.sinceId).toBe(3);
    expect(await quietFor(5_000)).toBe(true);

    await frame('delta', { messages: [row(4, 'assistant', 'Both are LRU')] });
    expect(text()).toContain('Both are LRU');
  });

  it('waits for a tick that is out before it takes frames', async () => {
    await mountPane();
    // A tick goes out and does not come back yet.
    let answer: (value: Awaited<ReturnType<typeof pollSessionMock>>) => void = () => {};
    pollSessionMock.mockImplementationOnce(
      () => new Promise((resolve) => (answer = resolve)) as ReturnType<typeof pollSessionMock>
    );
    await vi.advanceTimersByTimeAsync(500);
    expect(pollSessionMock).toHaveBeenCalledTimes(1);

    await establish();
    // Established, but the fetch that is out read the session before any
    // frame will have: the pane is not on the stream until it lands.
    expect(chat()).toBeUndefined();
    expect(await quietFor(2_000)).toBe(true);

    answer({
      session: session('running'),
      queuedMessages: [],
      messages: [row(2, 'assistant', 'The cache is'), row(3, 'tool_call', 'Read')],
      acpMetadataMessages: [],
    });
    await settle();
    await declared();
    expect(text()).toContain('The cache is');
    // And its cursor is what that fetch left it holding.
    expect(chat()?.cursor?.sinceId).toBe(3);
  });

  it('lets go of the stream while parked and catches up from its cursor when shown again', async () => {
    await establish();
    const { props, pane } = await mountPane();
    expect(chat()).toBeDefined();

    pane.park();
    props.active = false;
    flushSync();
    await declared();
    expect(lastInterest().sessions).toEqual([]);

    // Written while parked. A frame for it would find nobody.
    transcript.current = [...transcript.current, row(3, 'assistant', 'It is two maps')];

    pane.unpark();
    props.active = true;
    flushSync();
    await settle();
    await declared();
    // The reopen's one delta from where the pane was parked, then the stream
    // from what that left it holding.
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    expect(pollSessionMock.mock.calls[0][1].sinceId).toBe(2);
    expect(text()).toContain('It is two maps');
    expect(chat()?.cursor?.sinceId).toBe(3);
    expect(await quietFor(5_000)).toBe(true);

    // Rows that arrive from here on are the stream's alone: they must not
    // take the pane through its reopen again.
    const calls = link.interests.length;
    await frame('snapshot', { session: session('running'), queuedMessages: [] });
    await frame('delta', { messages: [row(4, 'assistant', 'Both are LRU')] });
    await frame('delta', { messages: [row(4, 'assistant', 'Both are LRU, sized apart')] });
    await declared();
    expect(text()).toContain('Both are LRU, sized apart');
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    expect(link.interests).toHaveLength(calls);
  });

  it('asks the stream, not the poll, for a re-fetch when reconcile goes idle', async () => {
    sessionRow.current = session('cancelled');
    await establish();
    await mountPane();
    await frame('snapshot', { session: session('cancelled'), queuedMessages: [] });
    const calls = link.interests.length;

    emit('session-reconcile-changed', { sessionId: SESSION_ID, phase: 'idle' });
    await declared();
    expect(pollSessionMock).not.toHaveBeenCalled();
    expect(link.interests).toHaveLength(calls + 1);
    expect(chat()?.cursor?.sinceId).toBe(2);

    // The backfill arrives as the snapshot.
    await frame('snapshot', {
      session: session('cancelled'),
      queuedMessages: [],
      messages: [row(2, 'assistant', 'The cache'), row(3, 'assistant', 'Stopped halfway.')],
    });
    expect(text()).toContain('Stopped halfway.');
  });

  it('feeds two panes showing one session from the same frames', async () => {
    await establish();
    const first = await mountPane();
    await frame('snapshot', { session: session('running'), queuedMessages: [] });
    await frame('delta', { messages: [row(3, 'assistant', 'It is two maps')] });

    // The second pane loads the transcript as it is by then.
    transcript.current = [...transcript.current, row(3, 'assistant', 'It is two maps')];
    const second = await mountPane();
    // One session in the interest, read again from the earlier cursor. Both
    // panes hold row 3, so that is where it starts.
    expect(lastInterest().sessions.filter((s) => s.level === 'full')).toHaveLength(1);
    expect(chat()?.cursor?.sinceId).toBe(3);

    await frame('snapshot', {
      session: session('running'),
      queuedMessages: [],
      messages: [row(3, 'assistant', 'It is two maps')],
    });
    await frame('delta', { messages: [row(4, 'assistant', 'Both are LRU')] });

    for (const { target } of [first, second]) {
      expect(target.textContent).toContain('It is two maps');
      expect(target.textContent).toContain('Both are LRU');
      // Rows the other pane's snapshot repeated merged as no-ops.
      expect(target.textContent?.match(/It is two maps/g)).toHaveLength(1);
    }
    expect(pollSessionMock).not.toHaveBeenCalled();

    // Closing one leaves the other on the stream.
    unmount(panes.pop()!);
    await declared();
    expect(chat()).toBeDefined();
    await frame('delta', { messages: [row(5, 'assistant', 'Done')] });
    expect(first.target.textContent).toContain('Done');
  });

  it('bounds the window after a snapshot that brings more than a page', async () => {
    await establish();
    const { target } = await mountPane();
    expect(rowCount(target)).toBe(2);

    // The window was hidden for a long while, say.
    const many = Array.from({ length: TRANSCRIPT_PAGE_ROWS + 40 }, (_, i) =>
      row(3 + i, i % 2 ? 'assistant' : 'user', `row ${3 + i}`)
    );
    await frame('snapshot', { session: session('running'), queuedMessages: [], messages: many });
    await settle();

    expect(rowCount(target)).toBe(TRANSCRIPT_PAGE_ROWS);
    expect(target.textContent).toContain(`row ${many[many.length - 1].id}`);
    expect(target.textContent).not.toContain('Explain the cache');
  });
});
