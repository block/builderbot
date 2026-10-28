import { describe, expect, it } from 'vitest';
import type { ProjectRepo } from '../../types';
import { attachedRepoKey, attachedRepoKeys, attachedRepoSignature } from './suggestedRepos';

function repo(overrides: Partial<ProjectRepo> = {}): ProjectRepo {
  return {
    id: 'r1',
    projectId: 'p1',
    githubRepo: 'org/alpha',
    branchName: 'main',
    subpath: null,
    isPrimary: true,
    reason: null,
    headRepo: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

function signatureFor(projectId: string, repos: ProjectRepo[]): string {
  return attachedRepoSignature(attachedRepoKeys(projectId, new Map(repos.map((r) => [r.id, r]))));
}

describe('attachedRepoKey', () => {
  it('appends a trimmed subpath and ignores blank ones', () => {
    expect(attachedRepoKey(repo())).toBe('org/alpha');
    expect(attachedRepoKey(repo({ subpath: '  pkg/a ' }))).toBe('org/alpha::pkg/a');
    expect(attachedRepoKey(repo({ subpath: '   ' }))).toBe('org/alpha');
  });
});

describe('attachedRepoSignature', () => {
  const own = [repo(), repo({ id: 'r2', githubRepo: 'org/beta', subpath: 'pkg' })];

  it("is unchanged when another project's repos hydrate", () => {
    const before = signatureFor('p1', own);
    const after = signatureFor('p1', [
      ...own,
      repo({ id: 'r3', projectId: 'p2', githubRepo: 'org/gamma' }),
    ]);
    expect(after).toBe(before);
  });

  it('is order independent', () => {
    expect(signatureFor('p1', own)).toBe(signatureFor('p1', [...own].reverse()));
  });

  it("changes when one of this project's repos is swapped", () => {
    const swapped = [repo(), repo({ id: 'r2', githubRepo: 'org/delta', subpath: 'pkg' })];
    expect(signatureFor('p1', swapped)).not.toBe(signatureFor('p1', own));
  });

  it('changes when a repo is added to or removed from this project', () => {
    expect(signatureFor('p1', [repo()])).not.toBe(signatureFor('p1', own));
    expect(signatureFor('p1', [])).toBe('');
  });
});
