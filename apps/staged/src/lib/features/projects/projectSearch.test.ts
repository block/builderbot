import { describe, expect, it } from 'vitest';
import type { Project } from '../../types';
import { searchProjects } from './projectSearch';

// ── Fixtures ──

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: 'p1',
    name: 'Alpha',
    githubRepo: 'org/alpha',
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
    statusOverride: null,
    ...overrides,
  };
}

const alpha = project({ id: 'p1', name: 'Alpha Service' });
const beta = project({ id: 'p2', name: 'beta-tools' });
const gamma = project({ id: 'p3', name: 'Gamma  Web' });
const all = [alpha, beta, gamma];

describe('searchProjects', () => {
  it('returns the same array for an empty query', () => {
    expect(searchProjects(all, '')).toBe(all);
  });

  it('treats a whitespace-only query as empty', () => {
    expect(searchProjects(all, '   ')).toBe(all);
  });

  it('matches by substring of the name, case-insensitively', () => {
    expect(searchProjects(all, 'ALPHA')).toEqual([alpha]);
    expect(searchProjects(all, 'tool')).toEqual([beta]);
    expect(searchProjects(all, 'a')).toEqual([alpha, beta, gamma]);
  });

  it('trims whitespace around the query before matching', () => {
    expect(searchProjects(all, '  beta ')).toEqual([beta]);
  });

  it('matches interior whitespace literally', () => {
    expect(searchProjects(all, 'gamma  web')).toEqual([gamma]);
    expect(searchProjects(all, 'gamma web')).toEqual([]);
  });

  it('returns an empty list when nothing matches', () => {
    expect(searchProjects(all, 'zeta')).toEqual([]);
  });

  it('does not match on the repo slug', () => {
    expect(searchProjects(all, 'org/')).toEqual([]);
  });

  it('preserves input order', () => {
    expect(searchProjects([gamma, alpha, beta], 'a')).toEqual([gamma, alpha, beta]);
  });
});
