import { describe, expect, it } from 'vitest';
import {
  createProjectOption,
  noteTransferCandidates,
  noteTransferLabels,
  repoAddedHint,
} from './noteTransferTarget';
import type { Project } from '../../types';

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
