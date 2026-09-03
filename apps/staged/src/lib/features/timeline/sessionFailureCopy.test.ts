import { describe, expect, it } from 'vitest';
import { failedArtifactSubtitle } from './sessionFailureCopy';

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
});
