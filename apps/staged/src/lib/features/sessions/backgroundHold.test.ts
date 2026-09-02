import { describe, expect, it } from 'vitest';
import { backgroundHoldTaskRows, isBackgroundHolding, liveActivityRow } from './backgroundHold';
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
