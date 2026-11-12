import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session, SessionMessage, SessionReconcileEvent } from '../../types';

/**
 * The chat pane's handling of a `disconnected` Blox session: the alert that
 * replaces Resume, Check now's wiring to the reconcile command, and the
 * notices a reconciled session ends with.
 */

const {
  sessionRow,
  transcript,
  eventListeners,
  reconcileSessionMock,
  cancelSessionMock,
  resumeSessionMock,
  pollSessionMock,
} = vi.hoisted(() => {
  const sessionRow = { current: null as Session | null };
  const transcript = { current: [] as SessionMessage[] };
  return {
    sessionRow,
    transcript,
    eventListeners: new Map<string, Set<(payload: unknown) => void>>(),
    reconcileSessionMock: vi.fn(async (_id: string) => {}),
    cancelSessionMock: vi.fn(async (_id: string) => {}),
    resumeSessionMock: vi.fn(async () => {}),
    pollSessionMock: vi.fn(async (_id: string, _request: unknown) => ({
      session: sessionRow.current,
      queuedMessages: [],
      messages: transcript.current,
      acpMetadataMessages: [],
    })),
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
  getLatestSessionMessages: async () => ({ data: transcript.current }),
  getSessionMessages: async () => ({ data: [] }),
  getFreshSessionMessages: async () => [],
  getSessionMessagesSince: async () => [],
  getSessionAcpMetadataMessages: async () => [],
  getSessionAcpMetadataMessagesSince: async () => [],
  listQueuedSessionMessages: async () => [],
  reconcileSession: reconcileSessionMock,
  cancelSession: cancelSessionMock,
  resumeSession: resumeSessionMock,
  pollSession: pollSessionMock,
}));

import SessionChatPane from './SessionChatPane.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';
import { disconnectedSessionsStore } from '../../stores/disconnectedSessions.svelte';
import { RECONCILE_MAY_STILL_RUN } from '../../shared/disconnectedSession';

const SESSION_ID = 'blox-session';

function row(overrides: Partial<Session> = {}): Session {
  return {
    id: SESSION_ID,
    status: 'disconnected',
    provider: null,
    workingDir: null,
    errorMessage: null,
    completionReason: null,
    ...overrides,
  } as unknown as Session;
}

function message(id: number, role: 'user' | 'assistant', content: string): SessionMessage {
  return { id, sessionId: SESSION_ID, role, content, createdAt: id * 1_000 } as SessionMessage;
}

function reconcileEvent(
  phase: SessionReconcileEvent['phase'],
  overrides: Partial<SessionReconcileEvent> = {}
): SessionReconcileEvent {
  return {
    sessionId: SESSION_ID,
    branchId: null,
    projectId: null,
    phase,
    reason: null,
    attempts: 1,
    nextAttemptAt: null,
    ...overrides,
  };
}

function emit(event: string, payload: unknown) {
  for (const listener of eventListeners.get(event) ?? []) listener(payload);
  flushSync();
}

let pane: ReturnType<typeof mount> | null = null;

async function mountPane() {
  const props = reactiveProps({ active: true, sessionId: SESSION_ID, hashtagItems: [] });
  const target = document.createElement('div');
  document.body.appendChild(target);
  pane = mount(SessionChatPane, { target, props });
  await settle();
}

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

const alert = () => document.querySelector('[data-testid="disconnected-session-alert"]');
const statusLine = () =>
  document.querySelector('[data-testid="disconnected-session-status"]')?.textContent?.trim() ??
  null;

function button(label: string): HTMLButtonElement | null {
  return (
    [...document.querySelectorAll<HTMLButtonElement>('button')].find(
      (b) => b.textContent?.trim() === label
    ) ?? null
  );
}

describe('SessionChatPane with a disconnected Blox session', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    transcript.current = [];
    eventListeners.clear();
    reconcileSessionMock.mockClear();
    cancelSessionMock.mockClear();
    resumeSessionMock.mockClear();
    pollSessionMock.mockClear();
    disconnectedSessionsStore.applyReconcileSnapshot([], () => false);
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    if (pane) unmount(pane);
    pane = null;
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('explains the lost connection, offering Check now and Stop but never Resume', async () => {
    // A resumable reason would otherwise offer Resume: a duplicate turn beside
    // the one that may still be running remotely.
    sessionRow.current = row({ completionReason: 'app_quit' });
    await mountPane();

    expect(alert()?.textContent).toContain(
      'Staged lost its connection to the workstation. The agent may still be working; Staged will catch up when it can reach Blox.'
    );
    expect(button('Check now')).not.toBeNull();
    expect(button('Stop')).not.toBeNull();
    expect(button('Resume')).toBeNull();
    expect(document.body.textContent).not.toMatch(/crashed/i);
  });

  it('asks the backend to reconcile on Check now, catching up until it answers', async () => {
    sessionRow.current = row();
    await mountPane();

    button('Check now')!.click();
    flushSync();
    expect(reconcileSessionMock).toHaveBeenCalledWith(SESSION_ID);
    expect(statusLine()).toBe('Catching up with the workstation…');
    expect(button('Check now')!.disabled).toBe(true);

    disconnectedSessionsStore.applyReconcileEvent(reconcileEvent('checking'));
    flushSync();
    expect(statusLine()).toBe('Catching up with the workstation…');

    disconnectedSessionsStore.applyReconcileEvent(
      reconcileEvent('waiting', { reason: { kind: 'unreachable', message: 'dial tcp: timeout' } })
    );
    flushSync();
    expect(statusLine()).toBe('Staged can’t reach Blox right now.');
    expect(button('Check now')!.disabled).toBe(false);
  });

  it('says so when the reconcile request itself fails', async () => {
    sessionRow.current = row();
    reconcileSessionMock.mockRejectedValueOnce(new Error('not a Blox session'));
    await mountPane();

    button('Check now')!.click();
    await settle();
    expect(statusLine()).toBeNull();
    expect(document.querySelector('[data-testid="composer-send-error"]')?.textContent).toBe(
      'Failed to check the workstation: not a Blox session'
    );
  });

  it('stops the session from the alert', async () => {
    sessionRow.current = row();
    await mountPane();

    button('Stop')!.click();
    await settle();
    expect(cancelSessionMock).toHaveBeenCalledWith(SESSION_ID);
  });

  it('flows back into the live view on reattach, then says it caught up', async () => {
    sessionRow.current = row();
    await mountPane();
    expect(alert()).not.toBeNull();

    emit('session-status-changed', { sessionId: SESSION_ID, status: 'running' });
    await settle();
    expect(alert()).toBeNull();
    expect(button('Resume')).toBeNull();

    sessionRow.current = row({ status: 'completed', completionReason: 'turn_complete' });
    emit('session-status-changed', {
      sessionId: SESSION_ID,
      status: 'completed',
      completionReason: 'turn_complete',
    });
    await settle();
    expect(document.body.textContent).toContain(
      'Staged reconnected to the workstation and caught up. The agent finished this turn.'
    );
  });

  describe('ties the caught-up note to the turn in flight at the disconnect', () => {
    // Rows the poll appends slide in; jsdom has no Web Animations, and the
    // slide itself is beside the point here.
    beforeEach(() => {
      Element.prototype.animate = () =>
        ({ onfinish: null, cancel() {}, playState: 'finished' }) as unknown as Animation;
    });
    afterEach(() => {
      delete (Element.prototype as Partial<Element>).animate;
    });

    async function settleAfter(laterRows: SessionMessage[]) {
      transcript.current = [message(1, 'user', 'Fix the bug'), message(2, 'assistant', 'On it')];
      sessionRow.current = row();
      await mountPane();
      expect(alert()).not.toBeNull();

      transcript.current = [...transcript.current, ...laterRows];
      sessionRow.current = row({ status: 'completed', completionReason: 'turn_complete' });
      emit('session-status-changed', {
        sessionId: SESSION_ID,
        status: 'completed',
        completionReason: 'turn_complete',
      });
      await settle();
    }

    it('says it caught up when that turn is the one that ended', async () => {
      await settleAfter([message(3, 'assistant', 'Fixed it')]);

      expect(document.body.textContent).toContain('Fixed it');
      expect(document.body.textContent).toContain(
        'Staged reconnected to the workstation and caught up. The agent finished this turn.'
      );
    });

    it('says nothing about the reconnect once a later turn has ended', async () => {
      // Reconcile settled the turn, and a queued follow-up drained from
      // another client ran its own turn to the end before this pane heard of
      // either.
      await settleAfter([message(3, 'user', 'Now add a test'), message(4, 'assistant', 'Added')]);

      expect(document.body.textContent).toContain('Now add a test');
      expect(document.body.textContent).not.toContain('Staged reconnected');
    });
  });

  it('re-fetches the session when reconcile goes idle for it, in case its settle was missed', async () => {
    sessionRow.current = row();
    await mountPane();

    // The row settled, but its status event never reached this pane.
    sessionRow.current = row({ status: 'completed', completionReason: 'turn_complete' });
    emit('session-reconcile-changed', reconcileEvent('idle', { sessionId: 'another-session' }));
    emit('session-reconcile-changed', reconcileEvent('checking'));
    await settle();
    expect(pollSessionMock).not.toHaveBeenCalled();

    emit('session-reconcile-changed', reconcileEvent('idle'));
    await settle();
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    expect(pollSessionMock.mock.calls[0][0]).toBe(SESSION_ID);
    expect(alert()).toBeNull();
    expect(document.body.textContent).toContain(
      'Staged reconnected to the workstation and caught up. The agent finished this turn.'
    );
  });

  it('re-fetches a stopped session when its backfill is done, after the row ended', async () => {
    // A Stop on a Blox turn ends the row first and backfills the stopped
    // turn's transcript after; reconcile going idle says that is done.
    // The row the poll appends slides in, and jsdom has no Web Animations.
    Element.prototype.animate = () =>
      ({ onfinish: null, cancel() {}, playState: 'finished' }) as unknown as Animation;
    try {
      sessionRow.current = row({ status: 'cancelled', completionReason: 'interrupted' });
      transcript.current = [message(1, 'user', 'Run the migration')];
      await mountPane();
      expect(pollSessionMock).not.toHaveBeenCalled();

      transcript.current = [message(2, 'assistant', 'Stopped the migration halfway.')];
      emit('session-reconcile-changed', reconcileEvent('idle'));
      await settle();

      expect(pollSessionMock).toHaveBeenCalledTimes(1);
      expect(document.body.textContent).toContain('Stopped the migration halfway.');
    } finally {
      delete (Element.prototype as Partial<Element>).animate;
    }
  });

  it('re-fetches a disconnected session on a slow timer until it settles', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    sessionRow.current = row();
    await mountPane();

    vi.advanceTimersByTime(14_999);
    expect(pollSessionMock).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    await settle();
    expect(pollSessionMock).toHaveBeenCalledTimes(1);
    expect(alert()).not.toBeNull();

    sessionRow.current = row({
      status: 'cancelled',
      completionReason: 'interrupted',
      errorMessage: RECONCILE_MAY_STILL_RUN,
    });
    vi.advanceTimersByTime(15_000);
    await settle();
    expect(pollSessionMock).toHaveBeenCalledTimes(2);
    expect(alert()).toBeNull();
    expect(button('Resume')).not.toBeNull();

    // Settled: no more re-fetching.
    vi.advanceTimersByTime(60_000);
    await settle();
    expect(pollSessionMock).toHaveBeenCalledTimes(2);
  });

  it('says nothing extra for a completed session it never saw disconnected', async () => {
    sessionRow.current = row({ status: 'completed', completionReason: 'turn_complete' });
    await mountPane();
    expect(document.body.textContent).not.toContain('Staged reconnected');
  });

  it('shows an unconfirmed stop as a resumable notice, not an error', async () => {
    sessionRow.current = row({
      status: 'cancelled',
      completionReason: 'interrupted',
      errorMessage: RECONCILE_MAY_STILL_RUN,
    });
    await mountPane();

    expect(document.body.textContent).toContain(
      'You stopped this session. The workstation didn’t confirm the stop, so the turn may still be running there.'
    );
    expect(document.body.textContent).not.toContain(RECONCILE_MAY_STILL_RUN);
    expect(button('Resume')).not.toBeNull();
  });
});
