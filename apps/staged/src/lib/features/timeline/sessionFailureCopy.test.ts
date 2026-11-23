import { describe, expect, it } from 'vitest';
import {
  failedArtifactSubtitle,
  formatPauseTime,
  formatRateLimitResetTime,
  rateLimitPausedSubtitle,
} from './sessionFailureCopy';
import {
  RECONCILE_MAY_STILL_RUN,
  RECONCILE_NO_WORKSPACE,
  RECONCILE_PROMPT_NOT_RECEIVED,
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_WORKSTATION_GONE,
} from '../../shared/disconnectedSession';

/** A local wall-clock time, so "today" does not depend on the machine's time zone. */
function local(month: number, day: number, hour: number, minute = 0): Date {
  return new Date(2026, month - 1, day, hour, minute);
}

const timeOnly = (date: Date) =>
  date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
const dateAndTime = (date: Date) =>
  date.toLocaleString([], { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });

describe('formatPauseTime', () => {
  const now = local(11, 23, 14);

  it('gives the time alone for a time later today', () => {
    const reset = local(11, 23, 17, 30);
    expect(formatPauseTime(reset, now)).toBe(timeOnly(reset));
    expect(formatPauseTime(reset.getTime(), now)).toBe(timeOnly(reset));
    // Earlier today too: the day is what matters, not the direction.
    expect(formatPauseTime(local(11, 23, 0, 5), now)).toBe(timeOnly(local(11, 23, 0, 5)));
  });

  it('puts the date ahead of the time for a weekly reset days away', () => {
    const reset = local(11, 27, 9);
    const formatted = formatPauseTime(reset, now);
    expect(formatted).toBe(dateAndTime(reset));
    expect(formatted).not.toBe(timeOnly(reset));
    expect(formatted).toContain('27');
  });

  it('dates a time just past midnight, hours away', () => {
    const reset = local(11, 24, 0, 30);
    expect(formatPauseTime(reset, local(11, 23, 23, 50))).toBe(dateAndTime(reset));
  });

  it('dates the same day of another month, and a day already gone', () => {
    expect(formatPauseTime(local(12, 23, 14), now)).toBe(dateAndTime(local(12, 23, 14)));
    expect(formatPauseTime(local(11, 22, 14), now)).toBe(dateAndTime(local(11, 22, 14)));
  });
});

describe('formatRateLimitResetTime', () => {
  const now = local(11, 23, 14);

  it('shows the date only when the reset is not today', () => {
    const today = local(11, 23, 17, 30);
    const nextWeek = local(11, 27, 9);
    expect(formatRateLimitResetTime(today.toISOString(), now)).toBe(timeOnly(today));
    expect(formatRateLimitResetTime(nextWeek.toISOString(), now)).toBe(dateAndTime(nextWeek));
  });

  it('is null without a usable time', () => {
    expect(formatRateLimitResetTime(null, now)).toBeNull();
    expect(formatRateLimitResetTime('', now)).toBeNull();
    expect(formatRateLimitResetTime('5:30pm (America/Los_Angeles)', now)).toBeNull();
  });

  it('reads the clock when no moment is given', () => {
    const inFiveDays = new Date(Date.now() + 5 * 24 * 60 * 60 * 1000);
    expect(formatRateLimitResetTime(inFiveDays.toISOString())).toBe(dateAndTime(inFiveDays));
    expect(rateLimitPausedSubtitle(inFiveDays.toISOString())).toBe(
      `Paused by API limit — resets around ${dateAndTime(inFiveDays)}`
    );
  });
});

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
