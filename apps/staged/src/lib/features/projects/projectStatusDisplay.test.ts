import { describe, expect, it } from 'vitest';
import type { Branch, Project } from '../../types';
import {
  DEFAULT_PROJECT_STATUS_OPTIONS,
  computedStatusLabel,
  isDefaultProjectStatusOptions,
  normalizeProjectStatusOptions,
  resolveComputedProjectStatus,
  resolveProjectStatus,
  resolvedStatusLabel,
  type ProjectStatusOption,
} from './projectStatusDisplay';

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

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'b1',
    projectId: 'p1',
    projectRepoId: null,
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: '/tmp/wt',
    createdAt: 0,
    updatedAt: 0,
    prState: null,
    prChecksStatus: null,
    prReviewDecision: null,
    prMergeable: null,
    prDraft: null,
    prUrl: null,
    prUpdatedAt: null,
    prFetchedAt: null,
    prHeadSha: null,
    ...overrides,
  };
}

const blocked = DEFAULT_PROJECT_STATUS_OPTIONS.find((o) => o.id === 'blocked')!;

// ── normalizeProjectStatusOptions ──

describe('normalizeProjectStatusOptions', () => {
  it('falls back (null) for absent or non-array values', () => {
    expect(normalizeProjectStatusOptions(undefined)).toBeNull();
    expect(normalizeProjectStatusOptions(null)).toBeNull();
    expect(normalizeProjectStatusOptions({ id: 'x' })).toBeNull();
    expect(normalizeProjectStatusOptions('done')).toBeNull();
  });

  it('respects an explicitly saved empty list', () => {
    expect(normalizeProjectStatusOptions([])).toEqual([]);
  });

  it('falls back when no entry survives validation', () => {
    expect(normalizeProjectStatusOptions([null, 3, { label: 'no id' }])).toBeNull();
  });

  it('drops malformed entries and keeps the rest in order', () => {
    const result = normalizeProjectStatusOptions([
      { id: 'a', label: 'A', icon: 'eye', color: 'red' },
      { id: '', label: 'empty id', icon: 'eye', color: 'red' },
      { id: 'b', label: 42, icon: 'eye', color: 'red' },
      'nope',
      { id: 'c', label: 'C', icon: 'ban', color: 'green' },
    ]);
    expect(result?.map((o) => o.id)).toEqual(['a', 'c']);
  });

  it('keeps the first of duplicate ids', () => {
    const result = normalizeProjectStatusOptions([
      { id: 'a', label: 'First', icon: 'eye', color: 'red' },
      { id: 'a', label: 'Second', icon: 'ban', color: 'green' },
    ]);
    expect(result).toEqual([{ id: 'a', label: 'First', icon: 'eye', color: 'red' }]);
  });

  it('coerces unknown colors to gray and missing icons to the default', () => {
    const result = normalizeProjectStatusOptions([{ id: 'a', label: 'A', color: '#ff0000' }]);
    expect(result).toEqual([{ id: 'a', label: 'A', icon: 'circle-dot', color: 'gray' }]);
  });

  it('keeps a blank label so an option being typed survives a reload', () => {
    const result = normalizeProjectStatusOptions([
      { id: 'a', label: '', icon: 'eye', color: 'red' },
    ]);
    expect(result?.[0].label).toBe('');
  });
});

// ── isDefaultProjectStatusOptions ──

describe('isDefaultProjectStatusOptions', () => {
  it('is true for the defaults, including an equal copy', () => {
    expect(isDefaultProjectStatusOptions(DEFAULT_PROJECT_STATUS_OPTIONS)).toBe(true);
    expect(
      isDefaultProjectStatusOptions(DEFAULT_PROJECT_STATUS_OPTIONS.map((o) => ({ ...o })))
    ).toBe(true);
  });

  it('is false once an option is added, removed, edited or reordered', () => {
    const defaults = DEFAULT_PROJECT_STATUS_OPTIONS;
    const added: ProjectStatusOption = { id: 'uuid', label: 'QA', icon: 'bug', color: 'amber' };
    expect(isDefaultProjectStatusOptions([...defaults, added])).toBe(false);
    expect(isDefaultProjectStatusOptions(defaults.slice(1))).toBe(false);
    expect(isDefaultProjectStatusOptions([])).toBe(false);
    expect(
      isDefaultProjectStatusOptions(
        defaults.map((o) => (o.id === 'blocked' ? { ...o, label: 'Stuck' } : o))
      )
    ).toBe(false);
    expect(
      isDefaultProjectStatusOptions(
        defaults.map((o) => (o.id === 'blocked' ? { ...o, color: 'gray' } : o))
      )
    ).toBe(false);
    expect(isDefaultProjectStatusOptions([...defaults].reverse())).toBe(false);
  });
});

// ── resolveProjectStatus ──

describe('resolveProjectStatus', () => {
  const options: ProjectStatusOption[] = DEFAULT_PROJECT_STATUS_OPTIONS;

  it('uses the PR status when no override is set', () => {
    const status = resolveProjectStatus(
      project(),
      [branch({ prNumber: 1, prState: 'MERGED' })],
      options
    );
    expect(status).toEqual({ kind: 'pr', prStatus: 'merged', hasCodeChanges: false });
  });

  it('lets an override win over the PR status', () => {
    const status = resolveProjectStatus(
      project({ statusOverride: 'blocked' }),
      [branch({ prNumber: 1, prState: 'MERGED' })],
      options
    );
    expect(status).toEqual({ kind: 'override', option: blocked });
  });

  it('lets an override win over the cloud status', () => {
    const status = resolveProjectStatus(
      project({ location: 'remote', statusOverride: 'blocked' }),
      [branch({ branchType: 'remote', workspaceStatus: 'running' })],
      options
    );
    expect(status).toEqual({ kind: 'override', option: blocked });
  });

  it('shows an override before branches are hydrated', () => {
    const status = resolveProjectStatus(project({ statusOverride: 'blocked' }), [], options, false);
    expect(status.kind).toBe('override');
  });

  it('treats an id with no matching option as Default', () => {
    const status = resolveProjectStatus(
      project({ statusOverride: 'deleted-option' }),
      [branch({ commitCount: 2 })],
      options
    );
    expect(status).toEqual({ kind: 'pr', prStatus: null, hasCodeChanges: true });
  });

  it('shows the cloud status for remote projects, hydrated or not', () => {
    const remote = project({ location: 'remote' });
    expect(
      resolveProjectStatus(remote, [branch({ workspaceStatus: 'starting' })], options)
    ).toEqual({ kind: 'cloud', workspaceStatus: 'starting' });
    expect(resolveProjectStatus(remote, [], options, false)).toEqual({
      kind: 'cloud',
      workspaceStatus: null,
    });
  });

  it('shows a placeholder for an unhydrated local project', () => {
    expect(resolveProjectStatus(project(), [], options, false)).toEqual({ kind: 'placeholder' });
  });

  it('treats a missing statusOverride (stale cached project) as Default', () => {
    const stale = { ...project(), statusOverride: undefined } as unknown as Project;
    expect(resolveProjectStatus(stale, [], options).kind).toBe('pr');
  });
});

// ── labels ──

describe('computedStatusLabel', () => {
  function labelFor(branches: Branch[]): string {
    return computedStatusLabel(resolveComputedProjectStatus(project(), branches));
  }

  it('labels each PR status', () => {
    expect(labelFor([branch({ prNumber: 1, prState: 'MERGED' })])).toBe('Merged');
    expect(labelFor([branch({ prNumber: 1, prState: 'OPEN' })])).toBe('PR open');
    expect(labelFor([branch({ prNumber: 1, prState: 'CLOSED' })])).toBe('PR closed');
    expect(labelFor([branch({ prNumber: 1, prMergeable: false })])).toBe('Merge conflict');
    expect(labelFor([branch({ prNumber: 1, prChecksStatus: 'FAILURE' })])).toBe('Checks failing');
    expect(labelFor([branch({ commitCount: 1 })])).toBe('No PR yet');
    expect(labelFor([])).toBe('No changes yet');
  });

  it('labels remote workspace states', () => {
    expect(computedStatusLabel({ kind: 'cloud', workspaceStatus: 'running' })).toBe(
      'Workspace running'
    );
    expect(computedStatusLabel({ kind: 'cloud', workspaceStatus: null })).toBe('Remote workspace');
  });
});

describe('resolvedStatusLabel', () => {
  it('uses the option label, falling back for a blank one', () => {
    expect(resolvedStatusLabel({ kind: 'override', option: blocked })).toBe('Blocked');
    expect(resolvedStatusLabel({ kind: 'override', option: { ...blocked, label: '  ' } })).toBe(
      'Untitled status'
    );
  });
});
