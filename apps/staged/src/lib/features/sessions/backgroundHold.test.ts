import { describe, expect, it } from 'vitest';
import {
  backgroundHoldTaskRows,
  isBackgroundHolding,
  liveActivityRow,
  nextBackgroundHold,
  pruneStoppingTaskIds,
} from './backgroundHold';
import { isResumableReason, RESUMABLE_REASONS } from '../../types';

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
});
