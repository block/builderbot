import { describe, expect, it, vi } from 'vitest';
import {
  childrenTravel,
  createProjectOption,
  lookedUpRepo,
  noteTransferCandidates,
  newRemoteProjectBlocker,
  noteTransferBlocker,
  noteTransferHint,
  noteTransferLabels,
  noteTransferTarget,
  repoAddedHint,
  transferFailedAfterCreate,
  unboundBranchNoteBlocker,
} from './noteTransferTarget';
import { filterMoveTargets } from '../branches/moveBranchTarget';
import type { Project } from '../../types';

// One project is shown under a name other than its `name` column, to check
// the picker's list and its "create project" row agree on what a project is
// called rather than each reading a different field.
vi.mock('../../shared/utils', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../shared/utils')>();
  return {
    ...actual,
    projectDisplayName: (p: Project) =>
      p.id === 'aliased' ? 'Shown Name' : actual.projectDisplayName(p),
  };
});

function project(overrides: Partial<Project> & Pick<Project, 'id' | 'name'>): Project {
  return {
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

const here = project({ id: 'here', name: 'Here' });
const there = project({ id: 'there', name: 'There' });

describe('noteTransferLabels', () => {
  it('names the action on the confirm button', () => {
    expect(noteTransferLabels('move')).toEqual({
      title: 'Move to project',
      confirm: 'Move',
      busy: 'Moving...',
    });
    expect(noteTransferLabels('copy')).toEqual({
      title: 'Copy to project',
      confirm: 'Copy',
      busy: 'Copying...',
    });
  });
});

describe('noteTransferCandidates', () => {
  it('leaves the current project off a move', () => {
    expect(noteTransferCandidates([here, there], 'here', 'move')).toEqual([there]);
  });

  it('offers the current project for a copy, which duplicates the note', () => {
    expect(noteTransferCandidates([here, there], 'here', 'copy')).toEqual([here, there]);
  });

  it('offers everything when the current project is unknown', () => {
    expect(noteTransferCandidates([here, there], null, 'move')).toEqual([here, there]);
  });

  it('leaves remote projects off when the note brings child notes along', () => {
    const remote = project({ id: 'remote', name: 'Remote', location: 'remote' });
    expect(
      noteTransferCandidates([here, there, remote], 'here', 'move', { localOnly: true })
    ).toEqual([there]);
    expect(noteTransferCandidates([here, there, remote], 'here', 'move')).toEqual([there, remote]);
  });
});

describe('childrenTravel', () => {
  it('is only true for a moved project note that has children', () => {
    expect(childrenTravel('move', 'project', true)).toBe(true);
    expect(childrenTravel('move', 'project', false)).toBe(false);
    expect(childrenTravel('copy', 'project', true)).toBe(false);
    expect(childrenTravel('move', 'branch', true)).toBe(false);
  });
});

describe('createProjectOption', () => {
  it('offers the trimmed query as a new project name', () => {
    expect(createProjectOption('  Payments  ', [here])).toBe('Payments');
  });

  it('offers nothing for an empty query', () => {
    expect(createProjectOption('   ', [here])).toBeNull();
  });

  it('does not offer a twin of an existing project, whatever the case', () => {
    expect(createProjectOption('here', [here])).toBeNull();
    expect(createProjectOption('HERE ', [here])).toBeNull();
  });

  it('judges a twin by the name the list shows, not the stored one', () => {
    const aliased = project({ id: 'aliased', name: 'stored-name' });
    const noRepos = new Map();

    // The query lands on the listed project, so no "create" row beside it.
    expect(filterMoveTargets([aliased], noRepos, 'shown name')).toEqual([aliased]);
    expect(createProjectOption('shown name', [aliased])).toBeNull();
    // The stored name is not what the user sees, so it is not a twin.
    expect(filterMoveTargets([aliased], noRepos, 'stored-name')).toEqual([]);
    expect(createProjectOption('stored-name', [aliased])).toBe('stored-name');
  });
});

describe('repoAddedHint', () => {
  it('spells out the repo and subpath a repo note brings along', () => {
    expect(repoAddedHint({ githubRepo: 'acme/widgets', subpath: 'apps/web' })).toBe(
      "acme/widgets (apps/web) is added to the project if it isn't there yet."
    );
    expect(repoAddedHint({ githubRepo: 'acme/widgets', subpath: null })).toBe(
      "acme/widgets is added to the project if it isn't there yet."
    );
  });

  it('says nothing for a project note', () => {
    expect(repoAddedHint(null)).toBeNull();
  });
});

describe('newRemoteProjectBlocker', () => {
  const repo = { githubRepo: 'acme/widgets', subpath: null };

  it('lets a repo note start a remote project from its repo', () => {
    expect(newRemoteProjectBlocker({ status: 'resolved', repo })).toBeNull();
  });

  it('keeps a note with no repo, like every project note, to a local project', () => {
    expect(newRemoteProjectBlocker({ status: 'resolved', repo: null })).toBe(
      'Remote needs a repo, and this note has none'
    );
  });

  it('waits for the lookup rather than claiming the note has no repo', () => {
    expect(newRemoteProjectBlocker({ status: 'loading' })).toBe(
      "Checking this note's repo for Remote…"
    );
    expect(newRemoteProjectBlocker({ status: 'failed' })).toBe(
      "Remote is unavailable: couldn't look up this note's repo"
    );
  });
});

describe('lookedUpRepo', () => {
  it('is the repo only once the lookup has found one', () => {
    const repo = { githubRepo: 'acme/widgets', subpath: 'apps/web' };
    expect(lookedUpRepo({ status: 'resolved', repo })).toBe(repo);
    expect(lookedUpRepo({ status: 'resolved', repo: null })).toBeNull();
    expect(lookedUpRepo({ status: 'loading' })).toBeNull();
    expect(lookedUpRepo({ status: 'failed' })).toBeNull();
  });
});

describe('noteTransferTarget', () => {
  it('is whatever was picked', () => {
    const picked = { kind: 'existing', projectId: 'here' } as const;
    expect(noteTransferTarget(picked, 'Fresh', [here, there], 'Fresh')).toBe(picked);
    expect(noteTransferTarget(picked, '', [here, there], null)).toBe(picked);
  });

  it('falls back to the only row the query leaves on offer', () => {
    expect(noteTransferTarget(null, 'There', [there], null)).toEqual({
      kind: 'existing',
      projectId: 'there',
    });
    expect(noteTransferTarget(null, 'Fresh', [], 'Fresh')).toEqual({ kind: 'new', name: 'Fresh' });
  });

  it('assumes nothing before anything is typed, even with one project on offer', () => {
    expect(noteTransferTarget(null, '', [there], null)).toBeNull();
    expect(noteTransferTarget(null, '  ', [there], null)).toBeNull();
  });

  it('is nothing while more than one row is on offer, or none', () => {
    expect(noteTransferTarget(null, 'e', [here, there], null)).toBeNull();
    expect(noteTransferTarget(null, 'The', [there], 'The')).toBeNull();
    expect(noteTransferTarget(null, 'Here', [], null)).toBeNull();
  });
});

describe('noteTransferBlocker', () => {
  const settled = { title: 'Plan', sessionId: 'session', completedAt: 1 };

  it('lets a settled note or one with no session go', () => {
    expect(noteTransferBlocker(settled, 'move')).toBeNull();
    expect(noteTransferBlocker({ ...settled, sessionId: null, completedAt: null }, 'copy')).toBe(
      null
    );
  });

  it('refuses a note its session is still writing, as the backend would', () => {
    expect(noteTransferBlocker({ ...settled, completedAt: null }, 'move')).toBe(
      '"Plan" is still being generated; wait for its session to finish before moving it.'
    );
    expect(noteTransferBlocker({ ...settled, title: ' ', completedAt: null }, 'copy')).toBe(
      'This note is still being generated; wait for its session to finish before copying it.'
    );
  });

  it('refuses a note that is no longer stored', () => {
    expect(noteTransferBlocker(null, 'move')).toBe('This note no longer exists.');
  });
});

describe('unboundBranchNoteBlocker', () => {
  it('lets a note bound to a repo go', () => {
    expect(unboundBranchNoteBlocker({ githubRepo: 'acme/widgets', subpath: null }, 'move')).toBe(
      null
    );
  });

  it('refuses a note whose branch has no repo, as the backend would', () => {
    expect(unboundBranchNoteBlocker(null, 'move')).toBe(
      "This note's branch is not bound to a repository, so the note cannot be moved to another project."
    );
    expect(unboundBranchNoteBlocker(null, 'copy')).toBe(
      "This note's branch is not bound to a repository, so the note cannot be copied to another project."
    );
  });
});

describe('transferFailedAfterCreate', () => {
  it('says the project was created before naming what failed', () => {
    expect(transferFailedAfterCreate('move', 'Fresh', 'disk full')).toBe(
      "Created Fresh, but the note wasn't moved: disk full"
    );
    expect(transferFailedAfterCreate('copy', 'Fresh', 'disk full')).toBe(
      "Created Fresh, but the note wasn't copied: disk full"
    );
  });
});

describe('noteTransferHint', () => {
  const repo = { githubRepo: 'acme/widgets', subpath: null };

  it('talks about the repo for a branch note, moved or copied', () => {
    expect(noteTransferHint('move', 'branch', repo)).toBe(repoAddedHint(repo));
    expect(noteTransferHint('copy', 'branch', repo)).toBe(repoAddedHint(repo));
  });

  it('says a moved project note takes its child notes but not its commits or reviews', () => {
    const hint = noteTransferHint('move', 'project', null, true);
    expect(hint).toContain('child notes and their sessions');
    expect(hint).toContain('Commits and code reviews stay on their branch');
  });

  it('says nothing for a project note without children, which moves alone', () => {
    expect(noteTransferHint('move', 'project', null, false)).toBeNull();
    expect(noteTransferHint('move', 'project', null)).toBeNull();
  });

  it('says nothing for a copied project note, which is standalone', () => {
    expect(noteTransferHint('copy', 'project', null, true)).toBeNull();
  });
});
