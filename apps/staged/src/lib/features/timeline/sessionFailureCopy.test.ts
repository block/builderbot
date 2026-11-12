import { describe, expect, it } from 'vitest';
import { failedArtifactSubtitle, formatRateLimitResetTime } from './sessionFailureCopy';
import {
  RECONCILE_MAY_STILL_RUN,
  RECONCILE_NO_WORKSPACE,
  RECONCILE_PROMPT_NOT_RECEIVED,
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_WORKSTATION_GONE,
} from '../../shared/disconnectedSession';

describe('failedArtifactSubtitle', () => {
  it('explains project-session interruptions', () => {
    expect(failedArtifactSubtitle('project_session_interrupted', 'commit')).toBe(
      'Session stopped by project session — no commit created'
    );
  });

  it('keeps direct interruptions generic in timeline rows', () => {
    expect(failedArtifactSubtitle('interrupted', 'note')).toBe('Session stopped — no note created');
  });

  it('names a truncated background wait', () => {
    expect(failedArtifactSubtitle('held_until_cap', 'commit')).toBe(
      'Session stopped waiting on background work — no commit created'
    );
    expect(failedArtifactSubtitle('hold_stopped', 'commit')).toBe(
      'Session stopped waiting on background work — no commit created'
    );
  });

  it('names a lost workspace agent', () => {
    expect(failedArtifactSubtitle('remote_agent_lost', 'commit')).toBe(
      'Workspace agent lost — no commit created'
    );
  });

  it('says a deleted workstation, not a crash, ended the session', () => {
    expect(failedArtifactSubtitle('crashed', 'commit', RECONCILE_WORKSTATION_GONE)).toBe(
      'Workstation deleted — no commit created'
    );
    expect(failedArtifactSubtitle('crashed', 'review', RECONCILE_WORKSTATION_GONE)).toBe(
      'Workstation deleted — no comments created'
    );
  });

  it('reads a turn Staged couldn’t follow as an interruption, not a Stop', () => {
    for (const message of [
      RECONCILE_TURN_UNDETERMINED,
      RECONCILE_NO_WORKSPACE,
      RECONCILE_PROMPT_NOT_RECEIVED,
    ]) {
      expect(failedArtifactSubtitle('interrupted', 'note', message)).toBe(
        'Session interrupted — no note created'
      );
    }
  });

  it('keeps a Stop the workstation didn’t confirm a Stop', () => {
    expect(failedArtifactSubtitle('interrupted', 'commit', RECONCILE_MAY_STILL_RUN)).toBe(
      'Session stopped — no commit created'
    );
  });

  it('falls back to the reason for any other message', () => {
    expect(failedArtifactSubtitle('crashed', 'commit', 'Blox process exited with code 1')).toBe(
      'Session crashed — no commit created'
    );
  });

  it('explains API limit failures', () => {
    expect(failedArtifactSubtitle('rate_limited', 'review')).toBe(
      'Paused by API limit — no comments yet'
    );
  });

  it('includes API limit reset time when available', () => {
    const resetTime = formatRateLimitResetTime('2026-06-24T15:45:00Z');

    expect(resetTime).toBeTruthy();
    expect(failedArtifactSubtitle('rate_limited', 'commit', null, '2026-06-24T15:45:00Z')).toBe(
      `Paused by API limit — resets around ${resetTime}`
    );
  });
});
