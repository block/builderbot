import { describe, expect, it } from 'vitest';
import {
  backgroundHoldTaskRows,
  isBackgroundHolding,
  isTaskHeld,
  knownSessionStatus,
  liveActivityRow,
  nextBackgroundHold,
  pruneStoppingTaskIds,
  pruneTaskStopNotices,
  statusEventSupersededLoad,
} from './backgroundHold';
import { isCompletedTurnReason, isResumableReason, RESUMABLE_REASONS } from '../../types';

describe('liveActivityRow', () => {
  it('shows the thinking row with a plain Stop before any hold is reported', () => {
    expect(liveActivityRow(null)).toEqual({
      label: 'Thinking…',
      stopLabel: 'Stop session',
      waitingOnBackground: false,
    });
    expect(liveActivityRow(undefined).label).toBe('Thinking…');
  });

  it('shows the wait and its live task count while holding', () => {
    expect(liveActivityRow({ holding: true, liveTasks: 1 })).toEqual({
      label: 'Waiting on background task (1)',
      stopLabel: 'Stop waiting and end session',
      waitingOnBackground: true,
    });
    expect(liveActivityRow({ holding: true, liveTasks: 3 }).label).toBe(
      'Waiting on background tasks (3)'
    );
  });

  it('keeps a Stop affordance in every state, so a wait is always escapable', () => {
    for (const hold of [
      null,
      { holding: true, liveTasks: 0 },
      { holding: true, liveTasks: 2 },
      { holding: false, liveTasks: 0 },
    ]) {
      expect(liveActivityRow(hold).stopLabel).toBeTruthy();
    }
  });

  it('names the wait without a count when the agent reports no tasks', () => {
    // The hold is real even with an empty set — with no raw-SDK stream to
    // confirm the background state it runs to its cap — but "(0)" would read
    // as though nothing were pending.
    expect(liveActivityRow({ holding: true, liveTasks: 0 }).label).toBe(
      'Waiting on background work'
    );
  });

  it('returns to thinking once the hold is withdrawn for a new turn', () => {
    expect(liveActivityRow({ holding: false, liveTasks: 2 })).toEqual({
      label: 'Thinking…',
      stopLabel: 'Stop session',
      waitingOnBackground: false,
    });
  });
});

describe('backgroundHoldTaskRows', () => {
  it('renders one stoppable row per named live task', () => {
    expect(
      backgroundHoldTaskRows({
        holding: true,
        liveTasks: 2,
        tasks: [
          {
            id: 'task-1',
            name: 'Run the tests',
            description: 'cargo test in the background',
            outputFilePath: '/tmp/tests.log',
          },
          { id: 'task-2', name: null, description: null, outputFilePath: null },
        ],
      })
    ).toEqual([
      {
        id: 'task-1',
        label: 'Run the tests',
        stopLabel: 'Stop "Run the tests"',
        description: 'cargo test in the background',
      },
      // A task that announced no name still gets a row — the id is the only
      // handle the user has on it.
      {
        id: 'task-2',
        label: 'Background task task-2',
        stopLabel: 'Stop "Background task task-2"',
        description: null,
      },
    ]);
  });

  it('is empty when the agent only reports a count (raw mode, older bridges)', () => {
    expect(backgroundHoldTaskRows({ holding: true, liveTasks: 2, tasks: [] })).toEqual([]);
    expect(backgroundHoldTaskRows({ holding: true, liveTasks: 2 })).toEqual([]);
  });

  it('is empty once the hold is withdrawn, whatever the last report carried', () => {
    expect(
      backgroundHoldTaskRows({
        holding: false,
        liveTasks: 0,
        tasks: [{ id: 'task-1', name: 'Run the tests', description: null, outputFilePath: null }],
      })
    ).toEqual([]);
    expect(backgroundHoldTaskRows(null)).toEqual([]);
    expect(backgroundHoldTaskRows(undefined)).toEqual([]);
  });
});

describe('isBackgroundHolding', () => {
  it('is true only for a live hold report', () => {
    expect(isBackgroundHolding({ holding: true, liveTasks: 0 })).toBe(true);
    expect(isBackgroundHolding({ holding: false, liveTasks: 0 })).toBe(false);
    expect(isBackgroundHolding(null)).toBe(false);
    expect(isBackgroundHolding(undefined)).toBe(false);
  });
});

describe('nextBackgroundHold', () => {
  const holding = {
    holding: true,
    liveTasks: 2,
    tasks: [{ id: 'task-1', name: 'Run the tests', description: null, outputFilePath: null }],
  };

  it('adopts a hold reported for a running session', () => {
    expect(nextBackgroundHold(holding, 'running')).toEqual(holding);
  });

  it('drops a hold reported after the session reached a terminal status', () => {
    // The event that arrives once the agent has torn down is stale: adopting
    // it would resurface the wait if the session later flips back to running
    // (a resume, a queued send), showing the old wait instead of "Thinking…".
    for (const status of ['completed', 'error', 'cancelled'] as const) {
      expect(nextBackgroundHold(holding, status)).toBeNull();
    }
  });

  it('keeps a hold for a session whose status is not loaded yet', () => {
    // On mount the snapshot request races the session load; the wait renders
    // behind its own running check, so dropping the hold here would lose the
    // very report the mounting pane asked for.
    expect(nextBackgroundHold(holding, null)).toEqual(holding);
    expect(nextBackgroundHold(holding, undefined)).toEqual(holding);
  });

  it('clears on a withdrawn hold whatever the status', () => {
    expect(nextBackgroundHold({ holding: false, liveTasks: 0 }, 'running')).toBeNull();
    expect(nextBackgroundHold(null, 'running')).toBeNull();
    expect(nextBackgroundHold(undefined, 'running')).toBeNull();
  });
});

describe('knownSessionStatus', () => {
  const loaded = { id: 'session-1', status: 'completed' as const };

  it('is the loaded status once the load for that session has resolved', () => {
    expect(knownSessionStatus('session-1', loaded, 'session-1')).toBe('completed');
  });

  it('is unknown while a reopened session still carries last time’s status', () => {
    // The reused-pane case: the pane kept the session object it was already
    // showing, so its status is whatever it was when the pane was last
    // visible — the load that would refresh it is still in flight.
    expect(knownSessionStatus('session-1', loaded, null)).toBeNull();
    expect(knownSessionStatus('session-1', loaded, undefined)).toBeNull();
  });

  it('is unknown when the loaded session is a different one', () => {
    expect(knownSessionStatus('session-2', loaded, 'session-1')).toBeNull();
    expect(knownSessionStatus('session-1', null, 'session-1')).toBeNull();
    expect(knownSessionStatus('session-1', undefined, 'session-1')).toBeNull();
  });

  it('is unknown when the current status belongs to a different session', () => {
    // Switched panes: the previous session's load is the one that resolved.
    expect(
      knownSessionStatus('session-1', { id: 'session-1', status: 'running' }, 'session-2')
    ).toBeNull();
  });
});

describe('a pane reopened on a session it already showed', () => {
  const holding = {
    holding: true,
    liveTasks: 1,
    tasks: [{ id: 'task-1', name: 'Run the tests', description: null, outputFilePath: null }],
  };
  // Left over from the last time the pane showed this session: it completed,
  // the user switched away, and it has since been resumed and is holding.
  const staleTerminal = { id: 'session-1', status: 'completed' as const };

  it('keeps the mount-time snapshot its stale terminal status would have discarded', () => {
    // The snapshot (one registry read) beats the session load (four queries,
    // one of them the whole transcript), and the event that would repaint the
    // wait is emit-on-change — so discarding this loses the wait for the rest
    // of the hold.
    expect(
      nextBackgroundHold(holding, knownSessionStatus('session-1', staleTerminal, null))
    ).toEqual(holding);
  });

  it('drops the same report once the load confirms the session really finished', () => {
    expect(
      nextBackgroundHold(holding, knownSessionStatus('session-1', staleTerminal, 'session-1'))
    ).toBeNull();
  });

  it('still drops a late holding report after a terminal status event', () => {
    // The guard's own job, unweakened: a status event applied to the loaded
    // session makes its status current too, so an event still in flight from
    // the torn-down agent can't resurface the wait on a later flip back to
    // running.
    for (const status of ['cancelled', 'error'] as const) {
      expect(
        nextBackgroundHold(
          holding,
          knownSessionStatus('session-1', { id: 'session-1', status }, 'session-1')
        )
      ).toBeNull();
    }
  });
});

describe('statusEventSupersededLoad', () => {
  const loaded = { id: 'session-1' };

  it('lets the load adopt its row when no status event moved', () => {
    expect(statusEventSupersededLoad(4, 4, loaded, 'session-1')).toBe(false);
    expect(statusEventSupersededLoad(4, 4, null, 'session-1')).toBe(false);
  });

  it('holds the load off when an event applied to the session mid-fetch', () => {
    expect(statusEventSupersededLoad(4, 5, loaded, 'session-1')).toBe(true);
  });

  it('lets a first load adopt its row even when an event moved', () => {
    // Nothing was loaded for that event to apply to, so it was dropped: the
    // fetched row is the only session this pane is going to get, and
    // discarding it would leave the pane with none at all.
    expect(statusEventSupersededLoad(4, 5, null, 'session-1')).toBe(false);
    expect(statusEventSupersededLoad(4, 5, undefined, 'session-1')).toBe(false);
  });

  it('lets the load adopt its row when the event applied to another session', () => {
    expect(statusEventSupersededLoad(4, 5, { id: 'session-2' }, 'session-1')).toBe(false);
  });
});

describe('a session that finishes while its pane is loading', () => {
  const holding = {
    holding: true,
    liveTasks: 1,
    tasks: [{ id: 'task-1', name: 'Run the tests', description: null, outputFilePath: null }],
  };

  it('keeps the event’s status rather than the row the load read before it', () => {
    // Ordering: the pane is reopened on a running session, `getSession` reads
    // `running`, the session then completes and its terminal event lands —
    // applying `completed` and marking the status current — and only then do
    // the load's four queries resolve.
    const versionBeforeFetch = 7;
    // What the terminal event left behind while the queries were in flight.
    const afterEvent = { id: 'session-1', status: 'completed' as const };
    const versionAfterFetch = 8;
    const markedCurrentByEvent = 'session-1';
    // What the load's `getSession` read, a moment before the session finished.
    const fetched = { id: 'session-1', status: 'running' as const };

    const adopt = !statusEventSupersededLoad(
      versionBeforeFetch,
      versionAfterFetch,
      afterEvent,
      fetched.id
    );
    expect(adopt).toBe(false);

    const shown = adopt ? fetched : afterEvent;
    const statusCurrentFor = adopt ? fetched.id : markedCurrentByEvent;
    expect(shown.status).toBe('completed');
    // With the event's status left in place and current, a late `holding: true`
    // is still judged against a finished session.
    expect(
      nextBackgroundHold(holding, knownSessionStatus('session-1', shown, statusCurrentFor))
    ).toBeNull();

    // The premise, so this can't pass by asserting nothing: adopting the
    // fetched row is exactly what would resurrect the wait.
    expect(
      nextBackgroundHold(holding, knownSessionStatus('session-1', fetched, fetched.id))
    ).toEqual(holding);
  });
});

describe('pruneStoppingTaskIds', () => {
  const hold = {
    holding: true,
    liveTasks: 2,
    tasks: [
      { id: 'task-1', name: 'Run the tests', description: null, outputFilePath: null },
      { id: 'task-2', name: 'Build docs', description: null, outputFilePath: null },
    ],
  };

  it('keeps a stop in flight while its row is still in the reported set', () => {
    // The agent publishing the task's terminal state is what proves the stop
    // took — until then the button must not re-enable and invite a re-click.
    expect(pruneStoppingTaskIds(new Set(['task-1']), hold)).toEqual(new Set(['task-1']));
  });

  it('releases a stop once its row leaves the set', () => {
    expect(
      pruneStoppingTaskIds(new Set(['task-1', 'task-2']), { ...hold, tasks: [hold.tasks[1]] })
    ).toEqual(new Set(['task-2']));
  });

  it('releases every stop when the hold is withdrawn', () => {
    expect(pruneStoppingTaskIds(new Set(['task-1']), { holding: false, liveTasks: 0 })).toEqual(
      new Set()
    );
    expect(pruneStoppingTaskIds(new Set(['task-1']), null)).toEqual(new Set());
  });

  it('releases a stop the agent never named — raw mode reports no tasks', () => {
    expect(pruneStoppingTaskIds(new Set(['task-1']), { holding: true, liveTasks: 2 })).toEqual(
      new Set()
    );
  });

  it('never grows the set', () => {
    expect(pruneStoppingTaskIds(new Set(), hold)).toEqual(new Set());
  });
});

describe('isTaskHeld', () => {
  const hold = {
    holding: true,
    liveTasks: 1,
    tasks: [{ id: 'task-1', name: 'Run the tests', description: null, outputFilePath: null }],
  };

  it('is true while the task still has a row on screen', () => {
    expect(isTaskHeld(hold, 'task-1')).toBe(true);
  });

  it('is false once the row has left the reported set', () => {
    // The stop the user asked for took effect while the request was in flight;
    // saying "did not stop" would contradict the row disappearing.
    expect(isTaskHeld({ ...hold, tasks: [] }, 'task-1')).toBe(false);
    expect(isTaskHeld(hold, 'task-2')).toBe(false);
  });

  it('is false for a withdrawn hold, or one that names nothing', () => {
    expect(isTaskHeld({ ...hold, holding: false }, 'task-1')).toBe(false);
    expect(isTaskHeld({ holding: true, liveTasks: 2 }, 'task-1')).toBe(false);
    expect(isTaskHeld(null, 'task-1')).toBe(false);
    expect(isTaskHeld(undefined, 'task-1')).toBe(false);
  });
});

describe('pruneTaskStopNotices', () => {
  const hold = {
    holding: true,
    liveTasks: 2,
    tasks: [
      { id: 'task-1', name: 'Run the tests', description: null, outputFilePath: null },
      { id: 'task-2', name: 'Build docs', description: null, outputFilePath: null },
    ],
  };
  const notices = new Map([
    ['task-1', "The agent didn't stop this task — it may have already finished."],
    ['task-2', 'Stop failed: connection closed'],
  ]);

  it('keeps a notice while the row it explains is still shown', () => {
    expect(pruneTaskStopNotices(notices, hold)).toEqual(notices);
  });

  it('drops a notice with the row it belongs to', () => {
    expect(pruneTaskStopNotices(notices, { ...hold, tasks: [hold.tasks[0]] })).toEqual(
      new Map([['task-1', notices.get('task-1')!]])
    );
  });

  it('drops every notice once the hold is withdrawn', () => {
    expect(pruneTaskStopNotices(notices, { holding: false, liveTasks: 0 })).toEqual(new Map());
    expect(pruneTaskStopNotices(notices, null)).toEqual(new Map());
  });
});

describe('held_until_cap', () => {
  it('is resumable so a session cut off mid-wait can be nudged', () => {
    expect(RESUMABLE_REASONS.has('held_until_cap')).toBe(true);
    expect(isResumableReason('held_until_cap')).toBe(true);
  });

  it('treats a stopped wait the same as a cap-truncated one', () => {
    expect(RESUMABLE_REASONS.has('hold_stopped')).toBe(true);
    expect(isResumableReason('hold_stopped')).toBe(true);
  });

  it('leaves a cleanly completed turn non-resumable', () => {
    expect(RESUMABLE_REASONS.has('turn_complete')).toBe(false);
    expect(isResumableReason('turn_complete')).toBe(false);
  });

  it('is a completed turn as well as a resumable one', () => {
    // The two sets overlap on purpose: a truncated wait leaves the turn both
    // complete (its output is real, so output-gated affordances belong) and
    // worth nudging (its background work went unconfirmed).
    for (const reason of ['held_until_cap', 'hold_stopped'] as const) {
      expect(isCompletedTurnReason(reason)).toBe(true);
      expect(isResumableReason(reason)).toBe(true);
    }
  });

  it('leaves reasons whose turn never finished out of the completed set', () => {
    expect(isCompletedTurnReason('turn_complete')).toBe(true);
    for (const reason of [
      'interrupted',
      'project_session_interrupted',
      'crashed',
      'app_quit',
      'unknown',
    ] as const) {
      expect(isCompletedTurnReason(reason)).toBe(false);
    }
    expect(isCompletedTurnReason(null)).toBe(false);
    expect(isCompletedTurnReason(undefined)).toBe(false);
  });
});
