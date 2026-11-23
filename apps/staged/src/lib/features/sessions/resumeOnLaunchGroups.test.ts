import { describe, expect, it } from 'vitest';
import type { InterruptedSession, LaunchRecovery } from '../../types';
import {
  defaultSelection,
  dispositionNote,
  groupByProject,
  interruptedSummary,
  resumableSessionIds,
  rowSelectable,
  selectedSessionIds,
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
      // A rebase pipeline: labelled by its kind, resumed like the rest.
      label: 'rebase on docs',
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
    session({
      sessionId: 'e',
      branchId: null,
      branchName: null,
      sessionType: 'note',
      label: 'note on Widgets',
      disposition: 'manual',
      reason: 'the project session never reached its agent',
    }),
  ],
};

describe('groupByProject', () => {
  it('gives each project one row, repo sessions before project sessions, in offer order', () => {
    const rows = groupByProject({
      repoSessions: [
        ...recovery.repoSessions,
        session({
          sessionId: 'f',
          projectId: 'p2',
          projectName: 'Gadgets',
          label: 'rebase on main',
        }),
      ],
      projectSessions: [
        ...recovery.projectSessions,
        session({ sessionId: 'g', projectId: 'p3', projectName: 'Gizmos', branchId: null }),
      ],
    });
    expect(rows.map((r) => r.name)).toEqual(['Widgets', 'Gadgets', 'Gizmos']);
    expect(rows.map((r) => r.sessions.map((s) => s.sessionId))).toEqual([
      ['a', 'b', 'c', 'd', 'e'],
      ['f'],
      ['g'],
    ]);
    // The manual project session is listed, not resumed.
    expect(rows[0].resumableIds).toEqual(['a', 'b', 'c', 'd']);
  });

  it('keeps a session whose project is gone', () => {
    const rows = groupByProject({
      repoSessions: [session({ projectId: null, branchName: null, projectName: null })],
      projectSessions: [],
    });
    expect(rows).toHaveLength(1);
    expect(rows[0].name).toBe('Unknown project');
  });
});

describe('selection', () => {
  const offer: LaunchRecovery = {
    repoSessions: [
      session({ sessionId: 'a', projectId: 'p1' }),
      session({ sessionId: 'b', projectId: 'p2', projectName: 'Gadgets' }),
      session({
        sessionId: 'c',
        projectId: 'p3',
        projectName: 'Gizmos',
        disposition: 'manual',
        reason: 'it never reached its agent',
      }),
    ],
    projectSessions: [session({ sessionId: 'd', projectId: 'p1', branchId: null })],
  };

  it('starts with every project that can be resumed checked', () => {
    expect(defaultSelection(offer)).toEqual(['project:p1', 'project:p2']);
  });

  it('shows a project with nothing resumable but leaves it unselectable', () => {
    const rows = groupByProject(offer);
    expect(rows.map(rowSelectable)).toEqual([true, true, false]);
  });

  it("sends the checked projects' resumable sessions, repo sessions first", () => {
    expect(selectedSessionIds(offer, defaultSelection(offer))).toEqual(['a', 'b', 'd']);
    expect(selectedSessionIds(offer, ['project:p2', 'project:p1'])).toEqual(['a', 'b', 'd']);
    expect(selectedSessionIds(offer, ['project:p2'])).toEqual(['b']);
  });

  it('sends nothing for an empty selection or a project that cannot be resumed', () => {
    expect(selectedSessionIds(offer, [])).toEqual([]);
    expect(selectedSessionIds(offer, ['project:p3', 'project:gone'])).toEqual([]);
  });
});

describe('resumableSessionIds', () => {
  it('leaves out sessions that are listed only', () => {
    expect(resumableSessionIds(recovery)).toEqual(['a', 'b', 'c', 'd']);
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
      'Staged was closed while 5 sessions were running or waiting. 4 of them can be restarted automatically.'
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
