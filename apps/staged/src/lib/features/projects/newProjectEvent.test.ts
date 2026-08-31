import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RepoFilterRef } from './projectFilters.svelte';
import type { NewProjectEventDetail } from './newProjectEvent';
import { repoSeedFromNewProjectEvent, repoSeedFromRepoFilters } from './newProjectEvent';

// The module reads the filter store singleton, whose runes don't exist under
// vitest — mock it so each test can drive the active repo filters directly.
const filters = vi.hoisted(() => ({ active: [] as RepoFilterRef[] }));

vi.mock('./projectFilters.svelte', () => ({
  projectFiltersStore: {
    get activeRepoFilters() {
      return filters.active;
    },
  },
}));

function newProjectEvent(detail?: NewProjectEventDetail): Event {
  return new CustomEvent('staged:new-project', { detail });
}

beforeEach(() => {
  filters.active = [];
});

describe('repoSeedFromRepoFilters', () => {
  it('seeds from a single active repo filter', () => {
    expect(repoSeedFromRepoFilters([{ repo: 'org/alpha', subpath: 'apps/web' }])).toEqual({
      nameWithOwner: 'org/alpha',
      subpath: 'apps/web',
    });
  });

  it('normalizes an empty subpath to undefined', () => {
    expect(repoSeedFromRepoFilters([{ repo: 'org/alpha', subpath: '' }])).toEqual({
      nameWithOwner: 'org/alpha',
      subpath: undefined,
    });
  });

  it('seeds nothing with no active repo filter', () => {
    expect(repoSeedFromRepoFilters([])).toBeNull();
  });

  it('seeds nothing with several active repo filters', () => {
    expect(
      repoSeedFromRepoFilters([
        { repo: 'org/alpha', subpath: '' },
        { repo: 'org/beta', subpath: '' },
      ])
    ).toBeNull();
  });
});

describe('repoSeedFromNewProjectEvent', () => {
  it('falls back to the single active repo filter when the event carries no repo', () => {
    filters.active = [{ repo: 'org/alpha', subpath: 'apps/web' }];
    expect(repoSeedFromNewProjectEvent(newProjectEvent())).toEqual({
      nameWithOwner: 'org/alpha',
      subpath: 'apps/web',
    });
  });

  it("prefers the event's repo over an active filter", () => {
    filters.active = [{ repo: 'org/alpha', subpath: '' }];
    expect(repoSeedFromNewProjectEvent(newProjectEvent({ githubRepo: 'org/beta' }))).toEqual({
      nameWithOwner: 'org/beta',
      subpath: undefined,
    });
  });

  it('seeds nothing when neither the event nor the filters name a repo', () => {
    expect(repoSeedFromNewProjectEvent(newProjectEvent())).toBeNull();
  });
});
