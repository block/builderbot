import { describe, expect, it } from 'vitest';
import type { Project } from '../../types';
import { searchProjects, visibleProjects, withStickyProject } from './projectSearch';

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

// Chips excluded beta: the typical "Unread" case after visiting it.
const chipsWithoutBeta = [alpha, gamma];

// The pre-search base: chip filters → sticky return-target card.
describe('withStickyProject', () => {
  it('returns the chip-filtered list unchanged without a sticky card', () => {
    expect(withStickyProject(all, chipsWithoutBeta, null)).toBe(chipsWithoutBeta);
  });

  it('returns the chip-filtered list unchanged when the chips include the sticky card', () => {
    expect(withStickyProject(all, chipsWithoutBeta, alpha.id)).toBe(chipsWithoutBeta);
  });

  it('re-inserts an excluded sticky card at its position in the full list', () => {
    expect(withStickyProject(all, chipsWithoutBeta, beta.id)).toEqual([alpha, beta, gamma]);
  });

  it('leaves the list alone when the sticky id is no longer a project', () => {
    expect(withStickyProject(all, chipsWithoutBeta, 'gone')).toEqual(chipsWithoutBeta);
  });

  // Drives the empty-state copy: the chips matched nothing, but the sticky
  // card was re-inserted, so when the query then hides it the query — not the
  // chips — is what emptied the grid.
  it('is non-empty when the chips matched nothing but the sticky card was re-inserted', () => {
    expect(withStickyProject(all, [], beta.id)).toEqual([beta]);
    expect(visibleProjects(all, [], beta.id, 'alpha')).toEqual([]);
  });
});

// The grid composition: chip filters → sticky return-target card → query.
describe('visibleProjects', () => {
  describe('without a sticky card', () => {
    it('returns the chip-filtered list unchanged for an empty query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, null, '')).toBe(chipsWithoutBeta);
    });

    it('applies the query to the chip-filtered list', () => {
      expect(visibleProjects(all, chipsWithoutBeta, null, 'gamma')).toEqual([gamma]);
      // beta matches the query but the chips excluded it.
      expect(visibleProjects(all, chipsWithoutBeta, null, 'beta')).toEqual([]);
    });
  });

  describe('with a sticky card the chips already include', () => {
    it('returns the chip-filtered list unchanged for an empty query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, alpha.id, '')).toBe(chipsWithoutBeta);
    });

    it('applies the query to the chip-filtered list', () => {
      expect(visibleProjects(all, chipsWithoutBeta, alpha.id, 'alpha')).toEqual([alpha]);
    });

    it('hides the sticky card when it does not match the query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, alpha.id, 'gamma')).toEqual([gamma]);
    });
  });

  describe('with a sticky card the chips excluded', () => {
    it('re-inserts it at its position in the full list', () => {
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, '')).toEqual([alpha, beta, gamma]);
    });

    it('re-inserts it and then applies the query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, 'beta')).toEqual([beta]);
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, 'a')).toEqual([alpha, beta, gamma]);
    });

    it('hides the sticky card when it does not match the query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, 'gamma')).toEqual([gamma]);
    });

    it('re-inserts it even when the chips matched nothing', () => {
      expect(visibleProjects(all, [], beta.id, '')).toEqual([beta]);
      expect(visibleProjects(all, [], beta.id, 'alpha')).toEqual([]);
    });

    it('leaves the list alone when the sticky id is no longer a project', () => {
      // A deleted return target: nothing to re-insert.
      expect(visibleProjects(all, chipsWithoutBeta, 'gone', '')).toEqual(chipsWithoutBeta);
    });
  });
});
