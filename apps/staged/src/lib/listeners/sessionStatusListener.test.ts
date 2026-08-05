// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';

/**
 * Busy-state hydration tests: the stores are hydrated from the
 * `get_active_sessions` snapshot (startup / reconnect / cache-stale), with
 * `session-status-changed` deltas applied on top.
 *
 * The rune-based stores can't be imported under plain vitest, so the registry
 * is replaced with a functional in-memory fake that mirrors the real
 * register/cleanup semantics.
 */

interface FakeMetadata {
  sessionId: string;
  projectId: string;
  branchId?: string;
  type: string;
  timestamp: number;
}

function createFakeRegistry(projectStateStore: {
  removeRunningSession: (projectId: string, sessionId: string) => void;
}) {
  const sessions = new Map<string, FakeMetadata>();
  return {
    sessions,
    register: vi.fn((sessionId: string, projectId: string, type: string, branchId?: string) => {
      sessions.set(sessionId, { sessionId, projectId, branchId, type, timestamp: Date.now() });
    }),
    getMetadata: (sessionId: string) => sessions.get(sessionId) ?? null,
    getAllSessionIds: () => Array.from(sessions.keys()),
    getProjectId: (sessionId: string) => sessions.get(sessionId)?.projectId ?? null,
    getBranchId: (sessionId: string) => sessions.get(sessionId)?.branchId ?? null,
    getType: (sessionId: string) => sessions.get(sessionId)?.type ?? null,
    unregister: vi.fn((sessionId: string) => {
      sessions.delete(sessionId);
    }),
    cleanupSession: vi.fn((sessionId: string) => {
      const projectId = sessions.get(sessionId)?.projectId;
      if (projectId) projectStateStore.removeRunningSession(projectId, sessionId);
      sessions.delete(sessionId);
    }),
    clear: () => sessions.clear(),
  };
}

/**
 * Workflow-store fake. The setters stay plain spies (tests assert on the
 * calls), but the two lookups the sweep depends on are backed by a real state
 * map so their validating semantics are exercised faithfully: a branch
 * resolves only for its own session id while still in the in-progress state.
 */
function createFakeWorkflowStore(
  registry: ReturnType<typeof createFakeRegistry>,
  inProgressState: 'creating' | 'pushing'
) {
  const states = new Map<string, { state: string; sessionId: string | null }>();
  return {
    states,
    getBranchIdForSession: vi.fn((sessionId: string) => {
      const branchId = registry.getBranchId(sessionId);
      if (!branchId) return null;
      const entry = states.get(branchId);
      return entry?.sessionId === sessionId && entry.state === inProgressState ? branchId : null;
    }),
    getSessionId: vi.fn((branchId: string) => states.get(branchId)?.sessionId ?? null),
  };
}

describe('sessionStatusListener busy-state hydration', () => {
  let getActiveSessions: ReturnType<typeof vi.fn>;
  let getSession: ReturnType<typeof vi.fn>;
  let invalidateBranchTimeline: ReturnType<typeof vi.fn>;
  let getFreshSessionMessages: ReturnType<typeof vi.fn>;
  let updateBranchPr: ReturnType<typeof vi.fn>;
  let refreshPrStatus: ReturnType<typeof vi.fn>;
  let clearBranchPrStatus: ReturnType<typeof vi.fn>;
  let listenToEvent: ReturnType<typeof vi.fn>;
  let unlistenEvents: ReturnType<typeof vi.fn>;
  let eventCallbacks: Map<string, (payload: unknown) => void>;
  let projectStateStore: {
    addRunningSession: ReturnType<typeof vi.fn>;
    removeRunningSession: Mock<(projectId: string, sessionId: string) => void>;
    markAsUnread: ReturnType<typeof vi.fn>;
  };
  let prStateStore: ReturnType<typeof createFakeWorkflowStore> & {
    clearSessionTracking: ReturnType<typeof vi.fn>;
    setPrCreated: ReturnType<typeof vi.fn>;
    setPrError: ReturnType<typeof vi.fn>;
    clearPrState: ReturnType<typeof vi.fn>;
    getPrState: ReturnType<typeof vi.fn>;
  };
  let pushStateStore: ReturnType<typeof createFakeWorkflowStore> & {
    clearSessionTracking: ReturnType<typeof vi.fn>;
    markQueuedPushStarted: ReturnType<typeof vi.fn>;
    setPushDone: ReturnType<typeof vi.fn>;
    setPushError: ReturnType<typeof vi.fn>;
    clearPushState: ReturnType<typeof vi.fn>;
    getPushState: ReturnType<typeof vi.fn>;
  };
  let sessionRegistry: ReturnType<typeof createFakeRegistry>;

  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers({ now: 1_000 });

    getActiveSessions = vi.fn().mockResolvedValue([]);
    getSession = vi.fn().mockResolvedValue(null);
    invalidateBranchTimeline = vi.fn();
    getFreshSessionMessages = vi.fn().mockResolvedValue([]);
    updateBranchPr = vi.fn().mockResolvedValue(undefined);
    refreshPrStatus = vi.fn().mockResolvedValue(undefined);
    clearBranchPrStatus = vi.fn().mockResolvedValue(undefined);
    unlistenEvents = vi.fn();
    eventCallbacks = new Map();
    listenToEvent = vi.fn((event: string, callback: (payload: unknown) => void) => {
      eventCallbacks.set(event, callback);
      return unlistenEvents;
    });
    projectStateStore = {
      addRunningSession: vi.fn(),
      removeRunningSession: vi.fn<(projectId: string, sessionId: string) => void>(),
      markAsUnread: vi.fn(),
    };
    sessionRegistry = createFakeRegistry(projectStateStore);
    prStateStore = {
      ...createFakeWorkflowStore(sessionRegistry, 'creating'),
      clearSessionTracking: vi.fn(),
      setPrCreated: vi.fn(),
      setPrError: vi.fn(),
      clearPrState: vi.fn(),
      getPrState: vi.fn().mockReturnValue(undefined),
    };
    pushStateStore = {
      ...createFakeWorkflowStore(sessionRegistry, 'pushing'),
      clearSessionTracking: vi.fn(),
      markQueuedPushStarted: vi.fn(),
      setPushDone: vi.fn(),
      setPushError: vi.fn(),
      clearPushState: vi.fn(),
      getPushState: vi.fn().mockReturnValue(undefined),
    };

    vi.doMock('../transport', () => ({ isTauri: true, listenToEvent }));
    vi.doMock('../commands', () => ({
      getActiveSessions,
      invalidateBranchTimeline,
      getSession,
      getFreshSessionMessages,
      updateBranchPr,
      refreshPrStatus,
      clearBranchPrStatus,
    }));
    vi.doMock('../features/layout/navigation.svelte', () => ({
      navigation: { selectedProjectId: null },
    }));
    vi.doMock('../stores/projectState.svelte', () => ({ projectStateStore }));
    vi.doMock('../stores/prState.svelte', () => ({ prStateStore }));
    vi.doMock('../stores/pushState.svelte', () => ({ pushStateStore }));
    vi.doMock('../stores/pullState.svelte', () => ({
      pullStateStore: {
        markQueuedPullStarted: vi.fn(),
        clearPullState: vi.fn(),
      },
    }));
    vi.doMock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));
    vi.doMock('../stores/sessionRegistry.svelte', () => ({ sessionRegistry }));
  });

  afterEach(() => {
    vi.doUnmock('../transport');
    vi.doUnmock('../commands');
    vi.doUnmock('../features/layout/navigation.svelte');
    vi.doUnmock('../stores/projectState.svelte');
    vi.doUnmock('../stores/prState.svelte');
    vi.doUnmock('../stores/pushState.svelte');
    vi.doUnmock('../stores/pullState.svelte');
    vi.doUnmock('svelte-sonner');
    vi.doUnmock('../stores/sessionRegistry.svelte');
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('registers unseen running sessions and applies event-path gating to the snapshot', async () => {
    getActiveSessions.mockResolvedValue([
      {
        sessionId: 'running-1',
        projectId: 'project-1',
        branchId: 'branch-1',
        sessionType: 'commit',
        status: 'running',
        isAutoReview: false,
      },
      {
        sessionId: 'queued-1',
        projectId: 'project-1',
        branchId: 'branch-1',
        sessionType: 'commit',
        status: 'queued',
        isAutoReview: false,
      },
      {
        sessionId: 'unresolved-1',
        projectId: null,
        branchId: null,
        sessionType: 'pr',
        status: 'running',
        isAutoReview: false,
      },
      {
        sessionId: 'auto-review-1',
        projectId: 'project-2',
        branchId: 'branch-2',
        sessionType: 'review',
        status: 'running',
        isAutoReview: true,
      },
      {
        sessionId: 'untyped-1',
        projectId: 'project-2',
        branchId: null,
        sessionType: null,
        status: 'running',
        isAutoReview: false,
      },
    ]);

    const { hydrateActiveSessions } = await import('./sessionStatusListener');
    await hydrateActiveSessions();

    expect(sessionRegistry.register.mock.calls).toEqual([
      ['running-1', 'project-1', 'commit', 'branch-1'],
      ['untyped-1', 'project-2', 'other', undefined],
    ]);
    expect(projectStateStore.addRunningSession.mock.calls).toEqual([
      ['project-1', 'running-1'],
      ['project-2', 'untyped-1'],
    ]);
  });

  it('keeps existing local metadata for sessions the snapshot also reports', async () => {
    // A pipeline (pr) session registered at launch with its real branch and
    // project — the snapshot reports it running but with the same id; local
    // knowledge must win.
    sessionRegistry.sessions.set('pr-1', {
      sessionId: 'pr-1',
      projectId: 'project-1',
      branchId: 'branch-1',
      type: 'pr',
      timestamp: 500,
    });
    getActiveSessions.mockResolvedValue([
      {
        sessionId: 'pr-1',
        projectId: 'project-1',
        branchId: null,
        sessionType: 'other',
        status: 'running',
        isAutoReview: false,
      },
    ]);

    const { hydrateActiveSessions } = await import('./sessionStatusListener');
    await hydrateActiveSessions();

    expect(sessionRegistry.register).not.toHaveBeenCalled();
    expect(sessionRegistry.cleanupSession).not.toHaveBeenCalled();
    expect(projectStateStore.addRunningSession).not.toHaveBeenCalled();
    expect(sessionRegistry.getMetadata('pr-1')).toMatchObject({ branchId: 'branch-1', type: 'pr' });
  });

  it('sweeps local entries the backend no longer reports as active', async () => {
    // A running entry whose terminal event this client missed — this is the
    // stuck-spinner case the snapshot heals.
    sessionRegistry.sessions.set('gone-1', {
      sessionId: 'gone-1',
      projectId: 'project-1',
      type: 'commit',
      timestamp: 500,
    });
    getActiveSessions.mockResolvedValue([]);

    const { hydrateActiveSessions } = await import('./sessionStatusListener');
    await hydrateActiveSessions();

    expect(sessionRegistry.cleanupSession).toHaveBeenCalledWith('gone-1');
    expect(projectStateStore.removeRunningSession).toHaveBeenCalledWith('project-1', 'gone-1');
    expect(sessionRegistry.getMetadata('gone-1')).toBeNull();
  });

  it('does not sweep entries registered while the snapshot fetch was in flight', async () => {
    sessionRegistry.sessions.set('stale-1', {
      sessionId: 'stale-1',
      projectId: 'project-1',
      type: 'commit',
      timestamp: 500,
    });
    vi.setSystemTime(2_000);
    getActiveSessions.mockImplementation(async () => {
      // An optimistic launch-site registration racing the fetch: the session
      // is newer than the snapshot, so the sweep must keep it.
      sessionRegistry.register('launched-mid-fetch', 'project-2', 'commit', 'branch-2');
      return [];
    });

    const { hydrateActiveSessions } = await import('./sessionStatusListener');
    await hydrateActiveSessions();

    expect(sessionRegistry.cleanupSession).toHaveBeenCalledWith('stale-1');
    expect(sessionRegistry.cleanupSession).not.toHaveBeenCalledWith('launched-mid-fetch');
    expect(sessionRegistry.getMetadata('launched-mid-fetch')).not.toBeNull();
  });

  it('does not re-register sessions whose terminal delta arrived while the fetch was in flight', async () => {
    const { listenForSessionStatus, hydrateActiveSessions } =
      await import('./sessionStatusListener');
    listenForSessionStatus();
    await vi.waitFor(() => expect(getActiveSessions).toHaveBeenCalledTimes(1));

    // A session running and registered via the delta path...
    eventCallbacks.get('session-status-changed')?.({
      sessionId: 'racy-1',
      status: 'running',
      projectId: 'project-1',
      branchId: 'branch-1',
      sessionType: 'commit',
    });
    expect(sessionRegistry.getMetadata('racy-1')).not.toBeNull();

    vi.setSystemTime(2_000);
    getActiveSessions.mockImplementation(async () => {
      // ...whose terminal delta lands while a snapshot that still reports it
      // running is in flight. The register loop must not resurrect it from
      // the stale snapshot — that would recreate the stuck spinner.
      eventCallbacks.get('session-status-changed')?.({
        sessionId: 'racy-1',
        status: 'completed',
        branchId: 'branch-1',
      });
      return [
        {
          sessionId: 'racy-1',
          projectId: 'project-1',
          branchId: 'branch-1',
          sessionType: 'commit',
          status: 'running',
          isAutoReview: false,
        },
      ];
    });

    sessionRegistry.register.mockClear();
    projectStateStore.addRunningSession.mockClear();
    await hydrateActiveSessions();

    expect(sessionRegistry.register).not.toHaveBeenCalled();
    expect(projectStateStore.addRunningSession).not.toHaveBeenCalled();
    expect(sessionRegistry.getMetadata('racy-1')).toBeNull();
  });

  it('lets a later hydration register a session terminated during an earlier fetch', async () => {
    const { listenForSessionStatus, hydrateActiveSessions } =
      await import('./sessionStatusListener');
    listenForSessionStatus();
    await vi.waitFor(() => expect(getActiveSessions).toHaveBeenCalledTimes(1));

    vi.setSystemTime(2_000);
    getActiveSessions.mockImplementation(async () => {
      eventCallbacks.get('session-status-changed')?.({
        sessionId: 'racy-1',
        status: 'completed',
        branchId: 'branch-1',
      });
      return [];
    });
    await hydrateActiveSessions();

    // The session was resumed backend-side after the terminal event: a fresh
    // snapshot legitimately reports it running again, and the guard from the
    // settled hydration must not suppress it.
    vi.setSystemTime(3_000);
    getActiveSessions.mockResolvedValue([
      {
        sessionId: 'racy-1',
        projectId: 'project-1',
        branchId: 'branch-1',
        sessionType: 'commit',
        status: 'running',
        isAutoReview: false,
      },
    ]);
    await hydrateActiveSessions();

    expect(sessionRegistry.register).toHaveBeenCalledWith(
      'racy-1',
      'project-1',
      'commit',
      'branch-1'
    );
    expect(sessionRegistry.getMetadata('racy-1')).not.toBeNull();
  });

  it('leaves state untouched when the snapshot fetch fails', async () => {
    sessionRegistry.sessions.set('running-1', {
      sessionId: 'running-1',
      projectId: 'project-1',
      type: 'commit',
      timestamp: 500,
    });
    getActiveSessions.mockRejectedValue(new Error('store not ready'));
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});

    const { hydrateActiveSessions } = await import('./sessionStatusListener');
    await hydrateActiveSessions();

    expect(sessionRegistry.cleanupSession).not.toHaveBeenCalled();
    expect(sessionRegistry.getMetadata('running-1')).not.toBeNull();
    expect(consoleError).toHaveBeenCalled();
  });

  it('hydrates on start, re-hydrates on cache-stale, and detaches on unlisten', async () => {
    const { listenForSessionStatus } = await import('./sessionStatusListener');

    const unlisten = listenForSessionStatus();
    expect(listenToEvent).toHaveBeenCalledWith('session-status-changed', expect.any(Function));
    expect(listenToEvent).toHaveBeenCalledWith('pr-created', expect.any(Function));
    expect(listenToEvent).toHaveBeenCalledWith('push-completed', expect.any(Function));
    await vi.waitFor(() => expect(getActiveSessions).toHaveBeenCalledTimes(1));

    window.dispatchEvent(new CustomEvent('cache-stale'));
    await vi.waitFor(() => expect(getActiveSessions).toHaveBeenCalledTimes(2));

    unlisten();
    expect(unlistenEvents).toHaveBeenCalledTimes(3);
    window.dispatchEvent(new CustomEvent('cache-stale'));
    expect(getActiveSessions).toHaveBeenCalledTimes(2);
  });

  it('still applies session-status-changed deltas on top of the snapshot', async () => {
    const { listenForSessionStatus } = await import('./sessionStatusListener');
    listenForSessionStatus();
    await vi.waitFor(() => expect(getActiveSessions).toHaveBeenCalledTimes(1));

    eventCallbacks.get('session-status-changed')?.({
      sessionId: 'delta-1',
      status: 'running',
      projectId: 'project-1',
      branchId: 'branch-1',
      sessionType: 'commit',
    });
    expect(sessionRegistry.register).toHaveBeenCalledWith(
      'delta-1',
      'project-1',
      'commit',
      'branch-1'
    );
    expect(projectStateStore.addRunningSession).toHaveBeenCalledWith('project-1', 'delta-1');

    eventCallbacks.get('session-status-changed')?.({
      sessionId: 'delta-1',
      status: 'completed',
      branchId: 'branch-1',
    });
    await vi.waitFor(() => expect(sessionRegistry.cleanupSession).toHaveBeenCalledWith('delta-1'));
    expect(invalidateBranchTimeline).toHaveBeenCalledWith('branch-1');
  });

  // -------------------------------------------------------------------------
  // Swept workflow chips: a client offline through a pipeline session's whole
  // completion missed both the domain event and the terminal event, so the
  // sweep reconciles its chip against the session's persisted status.
  // -------------------------------------------------------------------------

  describe('swept workflow reconciliation', () => {
    /** A pipeline session the snapshot will no longer report, chip still in progress. */
    function trackWorkflowSession(kind: 'pr' | 'push', sessionId: string, branchId = 'branch-1') {
      sessionRegistry.sessions.set(sessionId, {
        sessionId,
        projectId: 'project-1',
        branchId,
        type: kind,
        timestamp: 500,
      });
      const store = kind === 'pr' ? prStateStore : pushStateStore;
      store.states.set(branchId, { state: kind === 'pr' ? 'creating' : 'pushing', sessionId });
    }

    async function hydrate() {
      const { hydrateActiveSessions } = await import('./sessionStatusListener');
      await hydrateActiveSessions();
    }

    it('clears a swept pr chip whose session completed, letting the branch row drive it', async () => {
      trackWorkflowSession('pr', 'pr-1');
      getSession.mockResolvedValue({ id: 'pr-1', status: 'completed' });

      await hydrate();

      expect(getSession).toHaveBeenCalledWith('pr-1');
      // Not `setPrError`: the client missed `pr-created`, but the backend
      // persisted the PR number — a false "no PR URL found" would stick.
      expect(prStateStore.clearPrState).toHaveBeenCalledWith('branch-1');
      expect(prStateStore.setPrError).not.toHaveBeenCalled();
      expect(sessionRegistry.cleanupSession).toHaveBeenCalledWith('pr-1');
    });

    it('renders the delta-path error copy for a swept pr session that failed', async () => {
      trackWorkflowSession('pr', 'pr-1');
      getSession.mockResolvedValue({ id: 'pr-1', status: 'error' });

      await hydrate();

      expect(prStateStore.setPrError).toHaveBeenCalledWith(
        'branch-1',
        'PR creation session failed.'
      );
      expect(prStateStore.clearSessionTracking).toHaveBeenCalledWith('branch-1');
      expect(prStateStore.clearPrState).not.toHaveBeenCalled();
    });

    it('clears a swept push chip whose session completed without a done flash', async () => {
      trackWorkflowSession('push', 'push-1');
      getSession.mockResolvedValue({ id: 'push-1', status: 'completed' });

      await hydrate();

      expect(pushStateStore.clearPushState).toHaveBeenCalledWith('branch-1');
      expect(pushStateStore.setPushDone).not.toHaveBeenCalled();
      expect(pushStateStore.setPushError).not.toHaveBeenCalled();
    });

    it('renders the cancellation for a swept push session', async () => {
      trackWorkflowSession('push', 'push-1');
      getSession.mockResolvedValue({ id: 'push-1', status: 'cancelled' });

      await hydrate();

      expect(pushStateStore.setPushError).toHaveBeenCalledWith(
        'branch-1',
        'Push session was cancelled.'
      );
      expect(pushStateStore.clearSessionTracking).toHaveBeenCalledWith('branch-1');
      expect(pushStateStore.clearPushState).not.toHaveBeenCalled();
    });

    it('errors out a swept pr chip when the session has vanished', async () => {
      trackWorkflowSession('pr', 'pr-1');
      getSession.mockResolvedValue(null);

      await hydrate();

      expect(prStateStore.setPrError).toHaveBeenCalledWith(
        'branch-1',
        'Lost track of PR creation session.'
      );
      expect(prStateStore.clearSessionTracking).toHaveBeenCalledWith('branch-1');
    });

    it('leaves a swept chip in progress when the session lookup throws', async () => {
      trackWorkflowSession('pr', 'pr-1');
      getSession.mockRejectedValue(new Error('socket not ready'));
      const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});

      await hydrate();

      // A throw is a transport failure, not proof the session is gone — most
      // likely right after the WebSocket reconnect that triggered hydration.
      // The sticky "Lost track of…" error would outlive a PR that actually
      // succeeded, so keep the chip tracking for the card poller to heal.
      expect(prStateStore.setPrError).not.toHaveBeenCalled();
      expect(prStateStore.clearPrState).not.toHaveBeenCalled();
      expect(prStateStore.clearSessionTracking).not.toHaveBeenCalled();
      expect(consoleError).toHaveBeenCalled();
    });

    it('errors out a swept push chip when the session has vanished', async () => {
      trackWorkflowSession('push', 'push-1');
      getSession.mockResolvedValue(null);

      await hydrate();

      expect(pushStateStore.setPushError).toHaveBeenCalledWith(
        'branch-1',
        'Lost track of push session.'
      );
      expect(pushStateStore.clearSessionTracking).toHaveBeenCalledWith('branch-1');
    });

    it('leaves the chip alone when the backend resumed the session', async () => {
      trackWorkflowSession('pr', 'pr-1');
      getSession.mockResolvedValue({ id: 'pr-1', status: 'running' });

      await hydrate();

      expect(prStateStore.clearPrState).not.toHaveBeenCalled();
      expect(prStateStore.setPrError).not.toHaveBeenCalled();
      expect(prStateStore.clearSessionTracking).not.toHaveBeenCalled();
    });

    it('skips a chip already tracking a different session', async () => {
      trackWorkflowSession('pr', 'pr-1');
      // The user relaunched before hydration ran: the chip belongs to pr-2 now.
      prStateStore.states.set('branch-1', { state: 'creating', sessionId: 'pr-2' });

      await hydrate();

      expect(getSession).not.toHaveBeenCalled();
      expect(prStateStore.clearPrState).not.toHaveBeenCalled();
      expect(prStateStore.setPrError).not.toHaveBeenCalled();
    });

    it('skips a chip relaunched while the session lookup was in flight', async () => {
      trackWorkflowSession('pr', 'pr-1');
      getSession.mockImplementation(async () => {
        prStateStore.states.set('branch-1', { state: 'creating', sessionId: 'pr-2' });
        return { id: 'pr-1', status: 'completed' };
      });

      await hydrate();

      expect(prStateStore.clearPrState).not.toHaveBeenCalled();
      expect(prStateStore.setPrError).not.toHaveBeenCalled();
    });

    it('ignores swept sessions no workflow chip is tracking', async () => {
      sessionRegistry.sessions.set('commit-1', {
        sessionId: 'commit-1',
        projectId: 'project-1',
        branchId: 'branch-1',
        type: 'other',
        timestamp: 500,
      });

      await hydrate();

      expect(getSession).not.toHaveBeenCalled();
      expect(prStateStore.setPrError).not.toHaveBeenCalled();
      expect(pushStateStore.setPushError).not.toHaveBeenCalled();
      expect(sessionRegistry.cleanupSession).toHaveBeenCalledWith('commit-1');
    });
  });

  // -------------------------------------------------------------------------
  // Completion rendering: the backend parses/persists outcomes at the terminal
  // transition and emits `pr-created` / `push-completed` before the terminal
  // status event; these handlers only render and never write.
  // -------------------------------------------------------------------------

  describe('completion domain events', () => {
    async function listen() {
      const { listenForSessionStatus } = await import('./sessionStatusListener');
      listenForSessionStatus();
      await vi.waitFor(() => expect(getActiveSessions).toHaveBeenCalledTimes(1));
    }

    function registerSession(sessionId: string, type: string, branchId = 'branch-1') {
      sessionRegistry.sessions.set(sessionId, {
        sessionId,
        projectId: 'project-1',
        branchId,
        type,
        timestamp: 500,
      });
    }

    it('renders pr-created by marking the branch created', async () => {
      await listen();

      eventCallbacks.get('pr-created')?.({
        branchId: 'branch-1',
        sessionId: 'pr-1',
        prUrl: 'https://github.com/org/repo/pull/42',
        prNumber: 42,
      });

      expect(prStateStore.setPrCreated).toHaveBeenCalledWith(
        'branch-1',
        'https://github.com/org/repo/pull/42'
      );
    });

    it('renders push-completed success and clears it after the done flash', async () => {
      await listen();

      eventCallbacks.get('push-completed')?.({
        branchId: 'branch-1',
        sessionId: 'push-1',
        outcome: 'succeeded',
      });

      expect(pushStateStore.setPushDone).toHaveBeenCalledWith('branch-1');
      expect(pushStateStore.setPushError).not.toHaveBeenCalled();
      vi.advanceTimersByTime(1_500);
      expect(pushStateStore.clearPushState).toHaveBeenCalledWith('branch-1');
    });

    it('renders push-completed rejection as a non-fast-forward error', async () => {
      await listen();

      eventCallbacks.get('push-completed')?.({
        branchId: 'branch-1',
        sessionId: 'push-1',
        outcome: 'rejectedNonFastForward',
      });

      expect(pushStateStore.setPushError).toHaveBeenCalledWith('branch-1', '', true);
      expect(pushStateStore.setPushDone).not.toHaveBeenCalled();
    });

    it('leaves a created branch alone when the pr session completes', async () => {
      await listen();
      registerSession('pr-1', 'pr');
      prStateStore.getPrState.mockReturnValue({ state: 'created' });

      eventCallbacks.get('session-status-changed')?.({
        sessionId: 'pr-1',
        status: 'completed',
        branchId: 'branch-1',
      });

      expect(prStateStore.setPrError).not.toHaveBeenCalled();
      expect(prStateStore.clearSessionTracking).toHaveBeenCalledWith('branch-1');
    });

    it('reports a missing PR URL when completion arrives without pr-created', async () => {
      await listen();
      registerSession('pr-1', 'pr');

      eventCallbacks.get('session-status-changed')?.({
        sessionId: 'pr-1',
        status: 'completed',
        branchId: 'branch-1',
      });

      expect(prStateStore.setPrError).toHaveBeenCalledWith(
        'branch-1',
        'PR session completed but no PR URL was found in the output.'
      );
    });

    it('renders terminal pr failures and cancellations', async () => {
      await listen();
      registerSession('pr-1', 'pr');
      registerSession('pr-2', 'pr', 'branch-2');

      eventCallbacks.get('session-status-changed')?.({ sessionId: 'pr-1', status: 'error' });
      eventCallbacks.get('session-status-changed')?.({ sessionId: 'pr-2', status: 'cancelled' });

      expect(prStateStore.setPrError).toHaveBeenCalledWith(
        'branch-1',
        'PR creation session failed.'
      );
      expect(prStateStore.setPrError).toHaveBeenCalledWith(
        'branch-2',
        'PR creation session was cancelled.'
      );
    });

    it('falls back to success rendering when the push-completed event was missed', async () => {
      await listen();
      registerSession('push-1', 'push');
      pushStateStore.getPushState.mockReturnValue({ state: 'pushing' });

      eventCallbacks.get('session-status-changed')?.({
        sessionId: 'push-1',
        status: 'completed',
        branchId: 'branch-1',
      });

      expect(pushStateStore.setPushDone).toHaveBeenCalledWith('branch-1');
      vi.advanceTimersByTime(1_500);
      expect(pushStateStore.clearPushState).toHaveBeenCalledWith('branch-1');
    });

    it('does not re-render a push completion already handled by push-completed', async () => {
      await listen();
      registerSession('push-1', 'push');
      pushStateStore.getPushState.mockReturnValue({ state: 'done' });

      eventCallbacks.get('session-status-changed')?.({
        sessionId: 'push-1',
        status: 'completed',
        branchId: 'branch-1',
      });

      expect(pushStateStore.setPushDone).not.toHaveBeenCalled();
      expect(pushStateStore.setPushError).not.toHaveBeenCalled();
      expect(pushStateStore.clearSessionTracking).toHaveBeenCalledWith('branch-1');
    });

    it('renders terminal push failures and cancellations', async () => {
      await listen();
      registerSession('push-1', 'push');
      registerSession('push-2', 'push', 'branch-2');

      eventCallbacks.get('session-status-changed')?.({ sessionId: 'push-1', status: 'error' });
      eventCallbacks.get('session-status-changed')?.({ sessionId: 'push-2', status: 'cancelled' });

      expect(pushStateStore.setPushError).toHaveBeenCalledWith('branch-1', 'Push session failed.');
      expect(pushStateStore.setPushError).toHaveBeenCalledWith(
        'branch-2',
        'Push session was cancelled.'
      );
    });

    it('never performs authoritative writes from completion handling', async () => {
      await listen();
      registerSession('pr-1', 'pr');
      registerSession('push-1', 'push', 'branch-2');
      pushStateStore.getPushState.mockReturnValue({ state: 'pushing' });

      eventCallbacks.get('pr-created')?.({
        branchId: 'branch-1',
        sessionId: 'pr-1',
        prUrl: 'https://github.com/org/repo/pull/42',
        prNumber: 42,
      });
      eventCallbacks.get('push-completed')?.({
        branchId: 'branch-2',
        sessionId: 'push-1',
        outcome: 'succeeded',
      });
      eventCallbacks.get('session-status-changed')?.({
        sessionId: 'pr-1',
        status: 'completed',
        branchId: 'branch-1',
      });
      eventCallbacks.get('session-status-changed')?.({
        sessionId: 'push-1',
        status: 'completed',
        branchId: 'branch-2',
      });

      expect(updateBranchPr).not.toHaveBeenCalled();
      expect(refreshPrStatus).not.toHaveBeenCalled();
      expect(clearBranchPrStatus).not.toHaveBeenCalled();
      expect(getFreshSessionMessages).not.toHaveBeenCalled();
    });
  });
});
