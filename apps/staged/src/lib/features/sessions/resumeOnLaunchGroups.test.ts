import { describe, expect, it } from 'vitest';
import type { InterruptedSession, LaunchRecovery } from '../../types';
import {
  dispositionNote,
  groupInterruptedSessions,
  interruptedSummary,
  resumableSessionIds,
} from './resumeOnLaunchGroups';

function session(overrides: Partial<InterruptedSession>): InterruptedSession {
  return {
    sessionId: 's',
    projectId: 'p1',
    branchId: 'b1',
    sessionType: 'commit',
    label: 'commit on fix-login',
    branchName: 'fix-login',
    projectName: 'Widgets',
    disposition: 'resume',
    reason: null,
    interruptedAt: 0,
    ...overrides,
  };
}

const recovery: LaunchRecovery = {
  repoSessions: [
    session({ sessionId: 'a', branchId: 'b1' }),
    session({ sessionId: 'b', branchId: 'b1', disposition: 'restart' }),
    session({
      sessionId: 'c',
      branchId: 'b2',
      branchName: 'docs',
      sessionType: 'push',
      label: 'push on docs',
      disposition: 'manual',
      reason: 'pipeline sessions are not resumed automatically',
    }),
  ],
  projectSessions: [
    session({
      sessionId: 'd',
      branchId: null,
      branchName: null,
      sessionType: 'note',
      label: 'note on Widgets',
    }),
  ],
};

describe('groupInterruptedSessions', () => {
  it('groups repo sessions by branch, then project sessions by project, in offer order', () => {
    const groups = groupInterruptedSessions(recovery);
    expect(groups.map((g) => g.heading)).toEqual([
      'Widgets · fix-login',
      'Widgets · docs',
      'Widgets (project)',
    ]);
    expect(groups.map((g) => g.sessions.map((s) => s.sessionId))).toEqual([
      ['a', 'b'],
      ['c'],
      ['d'],
    ]);
  });

  it('keeps a session whose branch and project are gone', () => {
    const groups = groupInterruptedSessions({
      repoSessions: [session({ branchId: null, branchName: null, projectName: null })],
      projectSessions: [],
    });
    expect(groups).toHaveLength(1);
    expect(groups[0].heading).toBe('unknown branch');
  });
});

describe('resumableSessionIds', () => {
  it('leaves out sessions that are listed only', () => {
    expect(resumableSessionIds(recovery)).toEqual(['a', 'b', 'd']);
  });
});

describe('dispositionNote', () => {
  it('reads per disposition', () => {
    expect(dispositionNote(session({ disposition: 'resume' }))).toBeNull();
    expect(dispositionNote(session({ disposition: 'restart' }))).toBe(
      'will restart from the beginning'
    );
    expect(dispositionNote(session({ disposition: 'manual', reason: 'because' }))).toBe(
      'not restarted automatically: because'
    );
    expect(dispositionNote(session({ disposition: 'manual', reason: null }))).toBe('not restarted');
  });
});

describe('interruptedSummary', () => {
  it('counts and says how many can be restarted', () => {
    expect(interruptedSummary(recovery)).toBe(
      'Staged was closed while 4 sessions were running or waiting. 3 of them can be restarted automatically.'
    );
    expect(interruptedSummary({ repoSessions: [session({})], projectSessions: [] })).toBe(
      'Staged was closed while 1 session was running or waiting.'
    );
    expect(
      interruptedSummary({
        repoSessions: [session({ disposition: 'manual' })],
        projectSessions: [],
      })
    ).toBe(
      'Staged was closed while 1 session was running or waiting. None of them can be restarted automatically.'
    );
  });
});
