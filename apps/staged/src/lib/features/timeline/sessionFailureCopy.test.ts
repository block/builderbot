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

  it('says "yet" for a detached remote session only while Staged is still working on its note', () => {
    expect(failedArtifactSubtitle('detached', 'note', { isPollingForNote: true })).toBe(
      'Lost the remote session — no note created yet'
    );
    expect(failedArtifactSubtitle('detached', 'note', { isPollingForNote: false })).toBe(
      'Lost the remote session — no note created'
    );
    expect(failedArtifactSubtitle('detached', 'note')).toBe(
      'Lost the remote session — no note created'
    );
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
    expect(
      failedArtifactSubtitle('crashed', 'commit', { errorMessage: RECONCILE_WORKSTATION_GONE })
    ).toBe('Workstation deleted — no commit created');
    expect(
      failedArtifactSubtitle('crashed', 'review', { errorMessage: RECONCILE_WORKSTATION_GONE })
    ).toBe('Workstation deleted — no comments created');
  });

  it('reads a turn Staged couldn’t follow as an interruption, not a Stop', () => {
    for (const message of [
      RECONCILE_TURN_UNDETERMINED,
      RECONCILE_NO_WORKSPACE,
      RECONCILE_PROMPT_NOT_RECEIVED,
    ]) {
      expect(failedArtifactSubtitle('interrupted', 'note', { errorMessage: message })).toBe(
        'Session interrupted — no note created'
      );
    }
  });

  it('keeps a Stop the workstation didn’t confirm a Stop', () => {
    expect(
      failedArtifactSubtitle('interrupted', 'commit', { errorMessage: RECONCILE_MAY_STILL_RUN })
    ).toBe('Session stopped — no commit created');
  });

  it('falls back to the reason for any other message', () => {
    expect(
      failedArtifactSubtitle('crashed', 'commit', {
        errorMessage: 'Blox process exited with code 1',
      })
    ).toBe('Session crashed — no commit created');
  });

  it('explains API limit failures', () => {
    expect(failedArtifactSubtitle('rate_limited', 'review')).toBe(
      'Paused by API limit — no comments yet'
    );
  });

  it('includes API limit reset time when available', () => {
    const resetTime = formatRateLimitResetTime('2026-06-24T15:45:00Z');

    expect(resetTime).toBeTruthy();
    expect(
      failedArtifactSubtitle('rate_limited', 'commit', {
        rateLimitResetsAt: '2026-06-24T15:45:00Z',
      })
    ).toBe(`Paused by API limit — resets around ${resetTime}`);
  });

  it('keeps the API limit copy for an API-limit kind', () => {
    expect(failedArtifactSubtitle('rate_limited', 'note', { rateLimitKind: 'five_hour' })).toBe(
      'Paused by API limit — no note yet'
    );
    expect(failedArtifactSubtitle('rate_limited', 'note', { rateLimitKind: null })).toBe(
      'Paused by API limit — no note yet'
    );
  });

  // The row cannot know whether the scheduler gave up on the pause or will
  // not resume it by itself, so it must not say the session resumes.
  it('names a network pause without promising a resume, whatever the artifact', () => {
    for (const kind of ['commit', 'note', 'review'] as const) {
      const subtitle = failedArtifactSubtitle('rate_limited', kind, { rateLimitKind: 'network' });
      expect(subtitle).toBe('Paused: network unavailable.');
      expect(subtitle).not.toMatch(/resume/i);
    }
  });

  it('does not date a network pause by a provider reset time', () => {
    expect(
      failedArtifactSubtitle('rate_limited', 'commit', {
        rateLimitKind: 'network',
        rateLimitResetsAt: '2026-06-24T15:45:00Z',
      })
    ).toBe('Paused: network unavailable.');
  });

  it('reads the network kind only on a rate-limit pause', () => {
    expect(failedArtifactSubtitle('crashed', 'commit', { rateLimitKind: 'network' })).toBe(
      'Session crashed — no commit created'
    );
  });
});
