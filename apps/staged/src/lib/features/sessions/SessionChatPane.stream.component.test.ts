import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';
import type { SessionStreamHandlers } from '../../transport';
import type {
  Session,
  SessionFull,
  SessionLiveDelivery,
  SessionMessage,
  SessionMessageDelivery,
  SessionPollRequest,
  SessionStatus,
  SessionStreamInterest,
} from '../../types';

/**
 * The chat pane fed by the session stream: frames and no fetching on a timer
 * while the stream is established, nothing while it is being established,
 * and the fallback's `poll_session` tick once it has been given up on.
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

/** What the backend answers about delivering now, and to a send made now. */
const { liveDeliveryMock, sendNowMock, prefs } = vi.hoisted(() => ({
  liveDeliveryMock: vi.fn(async (_id: string): Promise<SessionLiveDelivery> => ({
    supported: false,
    retryable: false,
  })),
  sendNowMock: vi.fn(async (): Promise<SessionMessageDelivery> => ({ delivery: 'injected' })),
  prefs: { composerSendMode: 'queue' },
}));

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
  preferences: prefs,
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
  getSessionLiveDelivery: liveDeliveryMock,
  sendSessionMessageNow: sendNowMock,
  getLatestSessionMessages: async () => ({ data: transcript.current }),
  getSessionMessages: async () => ({ data: [] }),
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
import { LIVE_DELIVERY_RETRY_MS } from './liveDelivery';
import { FALLBACK_POLL_MS } from './messagePoll';
import { STATUS_HOLD_MS } from './statusHold';
import { TRANSCRIPT_PAGE_ROWS } from './transcriptWindow';
import {
  ESTABLISH_GRACE_MS,
  INTEREST_DELAY_MS,
  sessionStream,
} from '../../stores/sessionStream.svelte';

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
  /** How many times the fallback poll's interval has been started. */
  const ticking = () => intervals.mock.calls.filter(([, ms]) => ms === FALLBACK_POLL_MS).length;

  beforeEach(() => {
    document.body.innerHTML = '';
    eventListeners.clear();
    pollSessionMock.mockClear();
    liveDeliveryMock.mockReset();
    liveDeliveryMock.mockResolvedValue({ supported: false, retryable: false });
    sendNowMock.mockReset();
    prefs.composerSendMode = 'queue';
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

  /** Whether `ms` pass with no `poll_session` fetch. */
  async function quietFor(ms: number) {
    const calls = pollSessionMock.mock.calls.length;
    await vi.advanceTimersByTimeAsync(ms);
    await settle();
    return pollSessionMock.mock.calls.length === calls;
  }

  it('asks the stream for a running session from what it loaded, and fetches nothing', async () => {
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

  it('falls back to the poll while the stream is unavailable', async () => {
    // The stream could not be opened.
    expect(sessionStream.availability).toBe('unavailable');
    await mountPane();
    expect(link.interests).toHaveLength(0);
    // The load has just read the session, so the first fetch is a tick away.
    expect(pollSessionMock).not.toHaveBeenCalled();

    transcript.current = [...transcript.current, row(3, 'assistant', 'It is two maps')];
    expect(await quietFor(FALLBACK_POLL_MS / 2)).toBe(true);
    expect(await quietFor(FALLBACK_POLL_MS / 2)).toBe(false);
    expect(text()).toContain('It is two maps');
    expect(await quietFor(FALLBACK_POLL_MS)).toBe(false);
    expect(ticking()).toBe(1);
  });

  it('falls back for a session that is not running too, so its next turn arrives', async () => {
    sessionRow.current = session('completed');
    await mountPane();
    expect(thinking()).toBe(false);

    // Started from elsewhere. In the web build the status event travels on
    // the socket that is down, so nothing but the fetch can say so.
    sessionRow.current = session('running', { updatedAt: 2 });
    transcript.current = [...transcript.current, row(3, 'user', 'And the eviction?')];
    await vi.advanceTimersByTimeAsync(FALLBACK_POLL_MS);
    await settle();

    expect(thinking()).toBe(true);
    expect(text()).toContain('And the eviction?');
  });

  it('reads at once on a status event while it is on the fallback', async () => {
    await mountPane();
    await vi.advanceTimersByTimeAsync(FALLBACK_POLL_MS);
    await settle();
    const calls = pollSessionMock.mock.calls.length;

    // The turn ends: its final text was written just before the status.
    sessionRow.current = session('completed', { updatedAt: 2 });
    transcript.current = [transcript.current[0], row(2, 'assistant', 'The cache is two maps.')];
    emit('session-status-changed', { sessionId: SESSION_ID, status: 'completed' });
    await settle();

    expect(pollSessionMock).toHaveBeenCalledTimes(calls + 1);
    expect(text()).toContain('The cache is two maps.');
    expect(thinking()).toBe(false);
  });

  it('stops the fallback for a session that no longer exists', async () => {
    await mountPane();
    sessionRow.current = null;
    expect(await quietFor(FALLBACK_POLL_MS)).toBe(false);

    expect(await quietFor(FALLBACK_POLL_MS * 5)).toBe(true);
  });

  it('waits out a lost stream with what it shows, and is read again when it returns', async () => {
    await establish();
    await mountPane();
    await frame('snapshot', { session: session('running'), queuedMessages: [] });

    // The socket drops, and with it whatever had not been delivered.
    transcript.current = [...transcript.current, row(3, 'assistant', 'It is two maps')];
    link.handlers?.onClose();
    await settle();
    expect(sessionStream.availability).toBe('connecting');
    expect(await quietFor(ESTABLISH_GRACE_MS - 1_000)).toBe(true);
    expect(text()).toContain('The cache');
    expect(ticking()).toBe(0);

    // Back within the grace period: the pane never left the stream, and the
    // owner gives its cursor again for a snapshot of what it missed.
    link.handlers?.onOpen();
    await declared();
    expect(sessionStream.availability).toBe('established');
    expect(chat()?.cursor?.sinceId).toBe(2);
    await frame('snapshot', {
      session: session('running'),
      queuedMessages: [],
      messages: [row(2, 'assistant', 'The cache'), row(3, 'assistant', 'It is two maps')],
    });
    expect(text()).toContain('It is two maps');
    expect(pollSessionMock).not.toHaveBeenCalled();
  });

  it('goes to the poll when the stream is given up on, and back to the stream when it returns', async () => {
    await establish();
    await mountPane();
    expect(pollSessionMock).not.toHaveBeenCalled();

    transcript.current = [...transcript.current, row(3, 'assistant', 'It is two maps')];
    link.handlers?.onClose();
    await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS);
    await settle();
    expect(sessionStream.availability).toBe('unavailable');
    // One read from the cursor at once, then the tick.
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    expect(pollSessionMock.mock.calls[0][1].sinceId).toBe(2);
    expect(text()).toContain('It is two maps');
    expect(await quietFor(FALLBACK_POLL_MS)).toBe(false);
    expect(ticking()).toBe(1);

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

  it('opened while the stream is being established, waits for it and fetches nothing', async () => {
    sessionStream.start();
    expect(sessionStream.availability).toBe('connecting');
    await mountPane();
    expect(await quietFor(ESTABLISH_GRACE_MS - 1_000)).toBe(true);
    expect(ticking()).toBe(0);

    link.handlers?.onOpen();
    await declared();
    await declared();
    expect(chat()?.cursor?.sinceId).toBe(2);
    expect(pollSessionMock).not.toHaveBeenCalled();
  });

  it('opened while the stream is being established, reads at once when that is given up on', async () => {
    sessionStream.start();
    await mountPane();
    transcript.current = [...transcript.current, row(3, 'assistant', 'It is two maps')];

    await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS);
    await settle();

    // Nothing fed the pane while it waited, so it does not wait for a tick.
    expect(sessionStream.availability).toBe('unavailable');
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    expect(text()).toContain('It is two maps');
    expect(ticking()).toBe(1);
  });

  it('waits for a fetch that is out before it takes frames', async () => {
    await mountPane();
    // A tick goes out and does not come back yet.
    let answer: (value: Awaited<ReturnType<typeof pollSessionMock>>) => void = () => {};
    pollSessionMock.mockImplementationOnce(
      () => new Promise((resolve) => (answer = resolve)) as ReturnType<typeof pollSessionMock>
    );
    await vi.advanceTimersByTimeAsync(FALLBACK_POLL_MS);
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

  it('asks the stream for the slow re-fetch of a disconnected session', async () => {
    sessionRow.current = session('disconnected');
    await establish();
    await mountPane();
    await frame('snapshot', { session: session('disconnected'), queuedMessages: [] });
    const calls = link.interests.length;

    await vi.advanceTimersByTimeAsync(15_000);
    await declared();
    expect(link.interests).toHaveLength(calls + 1);
    expect(chat()?.cursor?.sinceId).toBe(2);
    expect(pollSessionMock).not.toHaveBeenCalled();

    // Reconcile settled it, and the status event was missed.
    await frame('snapshot', {
      session: session('completed', { updatedAt: 2 }),
      queuedMessages: [],
      messages: [row(2, 'assistant', 'The cache'), row(3, 'assistant', 'Caught up.')],
    });
    expect(text()).toContain('Caught up.');

    // Settled: nothing more is asked.
    const settled = link.interests.length;
    await vi.advanceTimersByTimeAsync(60_000);
    await declared();
    expect(link.interests).toHaveLength(settled);
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
  describe('live delivery', () => {
    const NOT_YET: SessionLiveDelivery = { supported: false, retryable: true, reason: 'not yet' };
    const SUPPORTED: SessionLiveDelivery = { supported: true, retryable: false };
    /**
     * Whether the composer offers "Send now". With the queue mode remembered
     * and no new-session actions, the control is split exactly then.
     */
    const offersSendNow = (target: Element) =>
      target.querySelector('[data-testid="prompt-input"]')?.getAttribute('data-send-split') ===
      'true';
    const asks = () => liveDeliveryMock.mock.calls.length;
    /** Let `ms` pass with no frame, and the asks made in it answer. */
    async function wait(ms: number) {
      await vi.advanceTimersByTimeAsync(ms);
      await settle();
    }

    it('asks again after a retryable no with no frame, and then offers the live send', async () => {
      liveDeliveryMock.mockResolvedValue(NOT_YET);
      await establish();
      const { target } = await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      expect(asks()).toBe(1);
      expect(offersSendNow(target)).toBe(false);

      // The agent is still connecting, and writes nothing.
      await wait(LIVE_DELIVERY_RETRY_MS - 1);
      expect(asks()).toBe(1);
      await wait(1);
      expect(asks()).toBe(2);
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(3);
      expect(offersSendNow(target)).toBe(false);

      // Connected, and still nothing written: the next ask finds out.
      liveDeliveryMock.mockResolvedValue(SUPPORTED);
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(4);
      expect(offersSendNow(target)).toBe(true);

      // A yes is final for the run.
      await wait(20 * LIVE_DELIVERY_RETRY_MS);
      await frame('delta', { messages: [row(3, 'assistant', 'It is two maps')] });
      expect(asks()).toBe(4);
      expect(pollSessionMock).not.toHaveBeenCalled();
    });

    it('does not ask again after a no that cannot change', async () => {
      await establish();
      const { target } = await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      expect(asks()).toBe(1);

      await wait(20 * LIVE_DELIVERY_RETRY_MS);
      await frame('delta', { messages: [row(3, 'assistant', 'It is two maps')] });
      expect(asks()).toBe(1);
      expect(offersSendNow(target)).toBe(false);
    });

    it('does not ask on a timer after a call that failed', async () => {
      liveDeliveryMock.mockRejectedValue(new Error('Session not found'));
      await establish();
      await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      expect(asks()).toBe(1);

      await wait(20 * LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(1);

      // The next update asks, as it always did.
      await frame('delta', { messages: [row(3, 'assistant', 'It is two maps')] });
      expect(asks()).toBe(2);
    });

    it('lets a frame ask in place of the timer, not as well as it', async () => {
      liveDeliveryMock.mockResolvedValue(NOT_YET);
      await establish();
      await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      expect(asks()).toBe(1);

      await wait(LIVE_DELIVERY_RETRY_MS - 100);
      await frame('delta', { messages: [row(3, 'assistant', 'It is two maps')] });
      expect(asks()).toBe(2);
      // The retry is counted from the frame's ask.
      await wait(100);
      expect(asks()).toBe(2);
      await wait(LIVE_DELIVERY_RETRY_MS - 100);
      expect(asks()).toBe(3);
    });

    it('asks at once after a live send was blocked, with no frame to prompt it', async () => {
      prefs.composerSendMode = 'send';
      liveDeliveryMock.mockResolvedValue(SUPPORTED);
      await establish();
      const { target } = await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      expect(asks()).toBe(1);
      expect(offersSendNow(target)).toBe(true);

      // The connection went under the send, and is being made again.
      sendNowMock.mockResolvedValue({ delivery: 'blocked', reason: 'connection closed' });
      liveDeliveryMock.mockResolvedValue(NOT_YET);
      const input = target.querySelector<HTMLElement>('[data-testid="prompt-input"]')!;
      input.textContent = 'Use the other map';
      input.dispatchEvent(new InputEvent('input', { bubbles: true }));
      flushSync();
      target.querySelector<HTMLElement>('[data-testid="composer-send"]')!.click();
      await settle();
      expect(sendNowMock).toHaveBeenCalledTimes(1);
      expect(target.textContent).toContain('Not delivered: connection closed');
      expect(asks()).toBe(2);
      expect(offersSendNow(target)).toBe(false);

      liveDeliveryMock.mockResolvedValue(SUPPORTED);
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(3);
      expect(offersSendNow(target)).toBe(true);
    });

    it('stops asking once the pane is parked', async () => {
      liveDeliveryMock.mockResolvedValue(NOT_YET);
      await establish();
      const { props, pane } = await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);

      pane.park();
      props.active = false;
      await settle();
      await wait(20 * LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);
    });

    it('stops asking once the pane is destroyed', async () => {
      liveDeliveryMock.mockResolvedValue(NOT_YET);
      await establish();
      await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);

      unmount(panes.pop()!);
      await wait(20 * LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);
    });

    it('stops asking once the pane shows another session', async () => {
      liveDeliveryMock.mockResolvedValue(NOT_YET);
      await establish();
      const { props } = await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);

      // The other session has ended, so there is nothing to ask about it.
      sessionRow.current = session('completed', { id: 'another-session' });
      transcript.current = [];
      props.sessionId = 'another-session';
      await settle();
      await wait(20 * LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);
    });

    it('stops asking when a status event ends the session', async () => {
      liveDeliveryMock.mockResolvedValue(NOT_YET);
      await establish();
      const { target } = await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);

      emit('session-status-changed', { sessionId: SESSION_ID, status: 'completed' });
      await settle();
      await wait(20 * LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);
      expect(offersSendNow(target)).toBe(false);
    });

    it('stops asking when a frame ends the session, and asks afresh for the next run', async () => {
      liveDeliveryMock.mockResolvedValue(NOT_YET);
      await establish();
      const { target } = await mountPane();
      await frame('snapshot', { session: session('running'), queuedMessages: [] });
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);

      await frame('delta', { session: session('completed', { updatedAt: 2 }) });
      await wait(20 * LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(2);

      // The next run: asked on its first frame, and again until it is a yes.
      await frame('delta', { session: session('running', { updatedAt: 3 }) });
      expect(asks()).toBe(3);
      liveDeliveryMock.mockResolvedValue(SUPPORTED);
      await wait(LIVE_DELIVERY_RETRY_MS);
      expect(asks()).toBe(4);
      expect(offersSendNow(target)).toBe(true);
    });
  });
});
