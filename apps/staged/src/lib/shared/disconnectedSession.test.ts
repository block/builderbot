import { describe, expect, it } from 'vitest';
import type { SessionReconcileEvent, SessionReconcileReasonKind } from '../types';
import {
  DISCONNECTED_ALERT_MESSAGE,
  disconnectedActivity,
  disconnectedAlertStatus,
  disconnectedLabel,
  isReconcileCancelNotice,
  nextCheckPhrase,
  RECONCILE_MAY_STILL_RUN,
  RECONCILE_NO_WORKSPACE,
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_WORKSTATION_GONE,
  representativeReconcileState,
} from './disconnectedSession';
import { projectSubtitle } from './utils';

const NOW = 1_000_000;

function state(
  phase: SessionReconcileEvent['phase'],
  overrides: Partial<SessionReconcileEvent> = {}
): SessionReconcileEvent {
  return {
    sessionId: 's1',
    branchId: 'b1',
    projectId: 'p1',
    phase,
    reason: null,
    attempts: 1,
    nextAttemptAt: null,
    ...overrides,
  };
}

function waiting(kind: SessionReconcileReasonKind, message = 'sq: exit 1', nextAttemptAt = null) {
  return state('waiting', { reason: { kind, message }, nextAttemptAt });
}

describe('disconnectedLabel', () => {
  it('reads as reconnecting before the backend has said anything', () => {
    expect(disconnectedLabel(null)).toBe('Running on Blox — reconnecting');
    expect(disconnectedLabel(state('waiting'))).toBe('Running on Blox — reconnecting');
  });

  it('says when Staged is catching up or stopping', () => {
    expect(disconnectedLabel(state('checking'))).toBe('Running on Blox — catching up…');
    expect(disconnectedLabel(state('stopping'))).toBe('Running on Blox — stopping…');
  });

  it('surfaces why the last attempt couldn’t catch up', () => {
    expect(disconnectedLabel(waiting('unreachable'))).toBe('Running on Blox — can’t reach Blox');
    expect(disconnectedLabel(waiting('not_authenticated'))).toBe(
      'Running on Blox — signed out of Blox'
    );
    expect(disconnectedLabel(waiting('suspended'))).toBe('Running on Blox — workstation suspended');
    expect(disconnectedLabel(waiting('workstation_not_running'))).toBe(
      'Running on Blox — workstation not running'
    );
    expect(disconnectedLabel(waiting('attach_failed'))).toBe(
      'Running on Blox — couldn’t reconnect'
    );
  });

  it('says when the turn is over but its background work is still running', () => {
    expect(disconnectedLabel(waiting('background_work'))).toBe(
      'Running on Blox — background work still running'
    );
  });

  it('never reads as a crash', () => {
    for (const s of [null, state('checking'), waiting('unreachable')]) {
      expect(disconnectedLabel(s)).not.toMatch(/crash/i);
    }
  });
});

describe('disconnectedActivity', () => {
  it('is the lowercase activity form, keeping Blox capitalised', () => {
    expect(disconnectedActivity(null)).toBe('running on Blox — reconnecting');
    expect(disconnectedActivity(waiting('suspended'))).toBe(
      'running on Blox — workstation suspended'
    );
  });

  it('fills an otherwise idle project subtitle, but never displaces live activity', () => {
    const activity = disconnectedActivity(null);
    expect(projectSubtitle(2, [], null, activity)).toBe('2 repos · running on Blox — reconnecting');
    expect(projectSubtitle(1, ['commit'], null, activity)).toBe('1 repo · making a commit');
    expect(projectSubtitle(1, [], null)).toBe('1 repo');
  });
});

describe('representativeReconcileState', () => {
  it('prefers a session in progress, then one with a reason', () => {
    const plain = state('waiting', { sessionId: 'plain' });
    const reasoned = waiting('unreachable');
    const checking = state('checking', { sessionId: 'checking' });
    expect(representativeReconcileState([plain, reasoned, checking])).toBe(checking);
    expect(representativeReconcileState([plain, undefined, reasoned])).toBe(reasoned);
    expect(representativeReconcileState([null, plain])).toBe(plain);
    expect(representativeReconcileState([null, undefined])).toBeNull();
  });
});

describe('disconnectedAlertStatus', () => {
  it('has no status line before the backend has said anything', () => {
    expect(disconnectedAlertStatus(null, NOW)).toEqual({ busy: null, text: null, detail: null });
  });

  it('shows catching up while a check runs, or as soon as one is requested', () => {
    const checking = { busy: 'checking', text: 'Catching up with the workstation…', detail: null };
    expect(disconnectedAlertStatus(state('checking'), NOW)).toEqual(checking);
    expect(disconnectedAlertStatus(null, NOW, true)).toEqual(checking);
    expect(disconnectedAlertStatus(waiting('unreachable'), NOW, true)).toEqual(checking);
  });

  it('shows a stop in progress over a requested check', () => {
    expect(disconnectedAlertStatus(state('stopping'), NOW, true)).toEqual({
      busy: 'stopping',
      text: 'Stopping the turn on the workstation…',
      detail: null,
    });
  });

  it('explains the reason with the raw error as detail, and when the next check is', () => {
    expect(
      disconnectedAlertStatus(
        state('waiting', {
          reason: { kind: 'suspended', message: 'workspace is suspended' },
          nextAttemptAt: NOW + 4 * 60_000,
        }),
        NOW
      )
    ).toEqual({
      busy: null,
      text: 'The workstation is suspended. Staged will catch up once it’s running again. Next check in 4 min.',
      detail: 'workspace is suspended',
    });
    expect(disconnectedAlertStatus(waiting('unreachable', ''), NOW)).toEqual({
      busy: null,
      text: 'Staged can’t reach Blox right now.',
      detail: null,
    });
  });

  it('explains a turn held for its background work, and when it is checked again', () => {
    const message =
      'The turn has ended, but background work it started is still running on the workstation';
    expect(
      disconnectedAlertStatus(
        state('waiting', {
          reason: { kind: 'background_work', message },
          attempts: 0,
          nextAttemptAt: NOW + 60_000,
        }),
        NOW
      )
    ).toEqual({
      busy: null,
      text: 'The turn has finished, but background work it started is still running on the workstation. Staged will catch up once it ends. Next check in under a minute.',
      detail: message,
    });
  });
});

describe('nextCheckPhrase', () => {
  it('rounds up to whole minutes, flooring at under a minute', () => {
    expect(nextCheckPhrase(null, NOW)).toBeNull();
    expect(nextCheckPhrase(NOW - 5_000, NOW)).toBe('Next check in under a minute.');
    expect(nextCheckPhrase(NOW + 30_000, NOW)).toBe('Next check in under a minute.');
    expect(nextCheckPhrase(NOW + 61_000, NOW)).toBe('Next check in 2 min.');
  });
});

describe('isReconcileCancelNotice', () => {
  it('recognises reconcile’s explanatory messages on cancelled rows only', () => {
    for (const message of [
      RECONCILE_MAY_STILL_RUN,
      RECONCILE_TURN_UNDETERMINED,
      RECONCILE_NO_WORKSPACE,
    ]) {
      expect(isReconcileCancelNotice('cancelled', message)).toBe(true);
      expect(isReconcileCancelNotice('error', message)).toBe(false);
    }
    expect(isReconcileCancelNotice('cancelled', RECONCILE_WORKSTATION_GONE)).toBe(false);
    expect(isReconcileCancelNotice('cancelled', 'generate_pikchr timed out')).toBe(false);
    expect(isReconcileCancelNotice('cancelled', null)).toBe(false);
  });
});

describe('DISCONNECTED_ALERT_MESSAGE', () => {
  it('says the agent may still be working and that Staged will catch up', () => {
    expect(DISCONNECTED_ALERT_MESSAGE).toBe(
      'Staged lost its connection to the workstation. The agent may still be working; Staged will catch up when it can reach Blox.'
    );
  });
});
