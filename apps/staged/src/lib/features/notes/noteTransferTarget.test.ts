import { describe, expect, it, vi } from 'vitest';
import {
  createProjectOption,
  noteTransferCandidates,
  noteTransferLabels,
  repoAddedHint,
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
    statusOverride: null,
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
