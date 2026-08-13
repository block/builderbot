import { describe, expect, it } from 'vitest';
import {
  branchRepoIdentity,
  filterMoveTargets,
  isMoveTargetChecking,
  moveTargetInvalidReason,
  nextMoveTargetIndex,
  repoKey,
} from './moveBranchTarget';
import type { Project, ProjectRepo } from '../../types';

function project(overrides: Partial<Project> & { id: string; name: string }): Project {
  return {
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

function repo(overrides: Partial<ProjectRepo> & { id: string; githubRepo: string }): ProjectRepo {
  return {
    projectId: 'p',
    branchName: 'main',
    subpath: null,
    isPrimary: false,
    reason: null,
    headRepo: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

describe('repoKey', () => {
  it('treats a null and an empty subpath as the same repo, like the unique index', () => {
    expect(repoKey('acme/widgets', null)).toBe(repoKey('acme/widgets', ''));
    expect(repoKey('acme/widgets', 'apps/web')).not.toBe(repoKey('acme/widgets', null));
  });
});

describe('branchRepoIdentity', () => {
  it('prefers the branch’s own repo row', () => {
    const identity = branchRepoIdentity(
      repo({ id: 'r1', githubRepo: 'acme/widgets', subpath: 'apps/web' }),
      project({ id: 'p1', name: 'Source', githubRepo: 'acme/other' })
    );

    expect(identity).toEqual({ githubRepo: 'acme/widgets', subpath: 'apps/web' });
  });

  it('falls back to the source project’s primary repo for a branch with no row', () => {
    const identity = branchRepoIdentity(
      null,
      project({ id: 'p1', name: 'Source', githubRepo: 'acme/widgets', subpath: 'apps/web' })
    );

    expect(identity).toEqual({ githubRepo: 'acme/widgets', subpath: 'apps/web' });
  });

  it('has no identity when neither the row nor the project names a repo', () => {
    expect(branchRepoIdentity(null, project({ id: 'p1', name: 'Empty' }))).toBeNull();
  });
});

describe('filterMoveTargets', () => {
  const alpha = project({ id: 'a', name: 'Alpha' });
  const beta = project({ id: 'b', name: 'Beta' });
  const repos = new Map<string, ProjectRepo[]>([
    ['a', [repo({ id: 'r1', githubRepo: 'acme/widgets' })]],
    ['b', [repo({ id: 'r2', githubRepo: 'other/gadgets', subpath: 'apps/web' })]],
  ]);

  it('returns every candidate for an empty query', () => {
    expect(filterMoveTargets([alpha, beta], repos, '  ')).toEqual([alpha, beta]);
  });

  it('matches on the project name', () => {
    expect(filterMoveTargets([alpha, beta], repos, 'bet')).toEqual([beta]);
  });

  it('matches on an attached repo path, including its subpath', () => {
    expect(filterMoveTargets([alpha, beta], repos, 'widgets')).toEqual([alpha]);
    expect(filterMoveTargets([alpha, beta], repos, 'gadgets/apps/web')).toEqual([beta]);
  });

  it('matches nothing when neither the name nor a repo matches', () => {
    expect(filterMoveTargets([alpha, beta], repos, 'zzz')).toEqual([]);
  });
});

describe('moveTargetInvalidReason', () => {
  const branchRepo = { githubRepo: 'acme/widgets', subpath: null };

  it('rejects a remote project, repos fetched or not', () => {
    const remote = project({ id: 'r', name: 'Remote', location: 'remote' });

    expect(moveTargetInvalidReason(remote, branchRepo, undefined)).toBe(
      "Remote projects can't receive branches."
    );
    expect(moveTargetInvalidReason(remote, branchRepo, [])).toBe(
      "Remote projects can't receive branches."
    );
  });

  it('rejects a project that already has the branch’s repo', () => {
    const target = project({ id: 't', name: 'Target' });

    expect(
      moveTargetInvalidReason(target, branchRepo, [repo({ id: 'r1', githubRepo: 'acme/widgets' })])
    ).toBe('Target already has acme/widgets attached.');
  });

  it('names the subpath when the branch’s repo has one', () => {
    const target = project({ id: 't', name: 'Target' });

    expect(
      moveTargetInvalidReason(target, { githubRepo: 'acme/widgets', subpath: 'apps/web' }, [
        repo({ id: 'r1', githubRepo: 'acme/widgets', subpath: 'apps/web' }),
      ])
    ).toBe('Target already has acme/widgets (apps/web) attached.');
  });

  it('counts an empty destination subpath as the same repo as a null one', () => {
    const target = project({ id: 't', name: 'Target' });

    expect(
      moveTargetInvalidReason(target, branchRepo, [
        repo({ id: 'r1', githubRepo: 'acme/widgets', subpath: '' }),
      ])
    ).toBe('Target already has acme/widgets attached.');
  });

  it('accepts a project holding the same repo at a different subpath', () => {
    const target = project({ id: 't', name: 'Target' });

    expect(
      moveTargetInvalidReason(target, branchRepo, [
        repo({ id: 'r1', githubRepo: 'acme/widgets', subpath: 'apps/web' }),
      ])
    ).toBeNull();
  });

  it('accepts a project with no overlapping repo', () => {
    const target = project({ id: 't', name: 'Target' });

    expect(
      moveTargetInvalidReason(target, branchRepo, [repo({ id: 'r1', githubRepo: 'other/gadgets' })])
    ).toBeNull();
  });
});

describe('nextMoveTargetIndex', () => {
  it('starts at the first row when nothing is selected', () => {
    expect(nextMoveTargetIndex(-1, 1, 3)).toBe(0);
    expect(nextMoveTargetIndex(-1, -1, 3)).toBe(0);
  });

  it('steps within the list and stops at both ends', () => {
    expect(nextMoveTargetIndex(0, 1, 3)).toBe(1);
    expect(nextMoveTargetIndex(2, 1, 3)).toBe(2);
    expect(nextMoveTargetIndex(1, -1, 3)).toBe(0);
    expect(nextMoveTargetIndex(0, -1, 3)).toBe(0);
  });

  it('clamps a cursor left past the end by a narrowing query', () => {
    expect(nextMoveTargetIndex(7, -1, 2)).toBe(1);
    expect(nextMoveTargetIndex(7, 1, 2)).toBe(1);
  });

  it('selects nothing when the query matched nothing', () => {
    expect(nextMoveTargetIndex(-1, 1, 0)).toBe(-1);
    expect(nextMoveTargetIndex(3, -1, 0)).toBe(-1);
  });
});

describe('isMoveTargetChecking', () => {
  it('waits on a local project whose repos have not been fetched', () => {
    expect(isMoveTargetChecking(project({ id: 't', name: 'Target' }), undefined)).toBe(true);
    expect(isMoveTargetChecking(project({ id: 't', name: 'Target' }), [])).toBe(false);
  });

  it('does not wait on a remote project — it is rejected either way', () => {
    expect(
      isMoveTargetChecking(project({ id: 'r', name: 'Remote', location: 'remote' }), undefined)
    ).toBe(false);
  });
});
