import { describe, expect, it } from 'vitest';
import type { Branch, Project, ProjectRepo } from '../../types';
import {
  matchProject,
  projectMatchReasons,
  projectShortcutNumbers,
  searchProjects,
  visibleProjects,
  withStickyProject,
  type ProjectSearchIndexItem,
  type SearchContext,
} from './projectSearch';
import { queryTerms } from '../../shared/textSearch';

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

  it('matches every whitespace-separated term, in any order', () => {
    const oneTwoThree = project({ id: 'p4', name: 'one two three' });
    const list = [...all, oneTwoThree];
    expect(searchProjects(list, 'one three')).toEqual([oneTwoThree]);
    expect(searchProjects(list, 'three one')).toEqual([oneTwoThree]);
    expect(searchProjects(list, 'one four')).toEqual([]);
    expect(searchProjects(list, 'two')).toEqual([oneTwoThree]);
  });

  it('is case-insensitive across every term', () => {
    const oneTwoThree = project({ id: 'p4', name: 'One Two THREE' });
    expect(searchProjects([...all, oneTwoThree], 'oNe tHree')).toEqual([oneTwoThree]);
  });

  it('ignores how the query and name space their words', () => {
    expect(searchProjects(all, 'gamma  web')).toEqual([gamma]);
    expect(searchProjects(all, 'gamma web')).toEqual([gamma]);
  });

  it('returns an empty list when nothing matches', () => {
    expect(searchProjects(all, 'zeta')).toEqual([]);
  });

  it('does not match on the repo slug without a context', () => {
    expect(searchProjects(all, 'org/')).toEqual([]);
  });

  it('preserves input order', () => {
    expect(searchProjects([gamma, alpha, beta], 'a')).toEqual([gamma, alpha, beta]);
  });
});

// ── Search context: branches, PRs, repo labels and index items ──

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'b1',
    projectId: 'p1',
    projectRepoId: null,
    branchName: 'main-work',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: null,
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

function projectRepo(overrides: Partial<ProjectRepo> = {}): ProjectRepo {
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

function indexItem(overrides: Partial<ProjectSearchIndexItem> = {}): ProjectSearchIndexItem {
  return { kind: 'note', id: 'n1', title: 'A note', updatedAt: 0, ...overrides };
}

function context(
  data: {
    branches?: Record<string, Branch[]>;
    repos?: Record<string, ProjectRepo[]>;
    shortNames?: Record<string, string>;
    index?: Record<string, ProjectSearchIndexItem[]>;
  } = {}
): SearchContext {
  return {
    branchesByProject: new Map(Object.entries(data.branches ?? {})),
    reposByProject: new Map(Object.entries(data.repos ?? {})),
    repoShortName: (githubRepo, subpath) =>
      data.shortNames?.[subpath ? `${githubRepo}/${subpath}` : githubRepo],
    index: data.index ? new Map(Object.entries(data.index)) : undefined,
  };
}

function match(p: Project, query: string, ctx?: SearchContext) {
  return matchProject(p, queryTerms(query), ctx);
}

describe('matchProject', () => {
  const app = project({ id: 'p1', name: 'Checkout', githubRepo: 'block/builderbot' });

  describe('identity: name and repo labels', () => {
    const ctx = context({
      repos: {
        p1: [
          projectRepo({ githubRepo: 'block/builderbot', subpath: 'apps/staged' }),
          projectRepo({ id: 'r2', githubRepo: 'upstream/widgets', headRepo: 'fork/widgets' }),
        ],
      },
      shortNames: { 'block/builderbot/apps/staged': 'stg' },
    });

    it('matches the repo path, subpath, fork head repo and badge short name', () => {
      expect(match(app, 'builderbot', ctx).matched).toBe(true);
      expect(match(app, 'apps/staged', ctx).matched).toBe(true);
      expect(match(app, 'fork/widgets', ctx).matched).toBe(true);
      expect(match(app, 'stg', ctx).matched).toBe(true);
      expect(match(app, 'nope', ctx).matched).toBe(false);
    });

    it('lets terms spread across the name and the repos', () => {
      expect(match(app, 'stg checkout', ctx).matched).toBe(true);
    });

    it('gives no reason: the name and badges on screen explain it', () => {
      expect(match(app, 'stg checkout', ctx)).toEqual({ matched: true, reason: null });
    });

    it("falls back to the project's own repo before its repos hydrate", () => {
      expect(match(app, 'builderbot', context()).matched).toBe(true);
    });

    it('searches only the name without a context', () => {
      expect(match(app, 'builderbot').matched).toBe(false);
      expect(match(app, 'checkout')).toEqual({ matched: true, reason: null });
    });
  });

  describe('branches and PR numbers', () => {
    const ctx = context({
      branches: {
        p1: [
          branch({ id: 'b1', branchName: 'fix-login-redirect', prNumber: 123, updatedAt: 1 }),
          branch({ id: 'b2', branchName: 'tidy-typos', updatedAt: 2 }),
          branch({ id: 'b3', branchName: 'issue-4567-crash', updatedAt: 3 }),
        ],
      },
    });

    it('matches a branch name, with the branch as the reason', () => {
      expect(match(app, 'typos', ctx)).toEqual({
        matched: true,
        reason: { kind: 'branch', label: 'tidy-typos', more: 0 },
      });
    });

    it('labels a branch with a PR by its number', () => {
      expect(match(app, 'redirect', ctx).reason).toEqual({
        kind: 'pr',
        label: '#123 · fix-login-redirect',
        more: 0,
      });
    });

    it('matches `#123` and `123` to PR 123 exactly', () => {
      expect(match(app, '#123', ctx).reason?.label).toBe('#123 · fix-login-redirect');
      expect(match(app, '123', ctx).reason?.label).toBe('#123 · fix-login-redirect');
    });

    it('does not match a prefix of a PR number', () => {
      expect(match(app, '12', ctx).matched).toBe(false);
      expect(match(app, '#12', ctx).matched).toBe(false);
    });

    it('still matches a number term as text', () => {
      expect(match(app, '456', ctx).reason?.label).toBe('issue-4567-crash');
    });

    it('requires every term not in the identity to land in one branch', () => {
      expect(match(app, 'login 123', ctx).matched).toBe(true);
      // "login" is one branch, "typos" another.
      expect(match(app, 'login typos', ctx).matched).toBe(false);
      // PR 123 is fix-login-redirect's, not tidy-typos'.
      expect(match(app, 'typos #123', ctx).matched).toBe(false);
    });

    it('pairs the identity with one branch', () => {
      expect(match(app, 'checkout typos', ctx).reason?.label).toBe('tidy-typos');
      expect(match(app, 'builderbot #123', ctx).reason?.kind).toBe('pr');
    });

    it('gives no reason when the identity alone matches, even if branches do too', () => {
      const named = project({ id: 'p1', name: 'Login work' });
      expect(match(named, 'login', ctx)).toEqual({ matched: true, reason: null });
    });

    it('shows the most recent of several matching branches and counts the rest', () => {
      // Every branch name has a "-"; the project's name and repo don't.
      expect(match(app, '-', ctx).reason).toEqual({
        kind: 'branch',
        label: 'issue-4567-crash',
        more: 2,
      });
    });

    it('cannot match on branches that have not hydrated', () => {
      expect(match(app, 'typos', context()).matched).toBe(false);
    });
  });

  describe('index items', () => {
    const ctx = context({
      branches: { p1: [branch({ branchName: 'login-page', updatedAt: 1 })] },
      index: {
        p1: [
          indexItem({ kind: 'review', id: 'v1', title: 'Review login flow', updatedAt: 9 }),
          indexItem({ kind: 'projectNote', id: 'pn1', title: 'Login plan', updatedAt: 8 }),
          indexItem({ kind: 'note', id: 'n1', title: 'Login notes', updatedAt: 2 }),
          indexItem({ kind: 'note', id: 'n2', title: 'Login research', updatedAt: 3 }),
          indexItem({ kind: 'note', id: 'n3', title: 'Fix the redirect', updatedAt: 4 }),
          indexItem({ kind: 'note', id: 'n4', title: 'PR #123: Fix login', updatedAt: 5 }),
        ],
      },
    });

    it('ranks branches over notes over project notes over reviews', () => {
      expect(match(app, 'login', ctx).reason).toEqual({
        kind: 'branch',
        label: 'login-page',
        more: 5,
      });
      expect(match(app, 'plan', ctx).reason?.kind).toBe('projectNote');
      expect(match(app, 'flow', ctx).reason?.kind).toBe('review');
    });

    it('breaks ties within a kind by the most recent', () => {
      // Only the notes "Login notes" and "Login research" hold both terms.
      expect(match(app, 'login es', ctx).reason).toEqual({
        kind: 'note',
        label: 'Login research',
        more: 1,
      });
    });

    it('does not pair terms from two different items', () => {
      // "redirect" is in n3, "plan" in pn1.
      expect(match(app, 'redirect plan', ctx).matched).toBe(false);
    });

    it('matches a number term against titles as text', () => {
      expect(match(app, '#123', ctx).reason?.label).toBe('PR #123: Fix login');
    });
  });
});

describe('searchProjects with a context', () => {
  const ctx = context({
    branches: {
      p1: [branch({ branchName: 'add-metrics', prNumber: 42 })],
      p3: [branch({ projectId: 'p3', branchName: 'metrics-dashboard' })],
    },
  });

  it('keeps the input order whatever each project matched on', () => {
    expect(searchProjects([gamma, beta, alpha], 'metrics', ctx)).toEqual([gamma, alpha]);
    expect(searchProjects(all, 'metrics', ctx)).toEqual([alpha, gamma]);
  });

  it('still matches names', () => {
    expect(searchProjects(all, 'beta', ctx)).toEqual([beta]);
  });

  it('is applied by visibleProjects', () => {
    expect(visibleProjects(all, all, null, '#42', ctx)).toEqual([alpha]);
    expect(visibleProjects(all, all, null, '#42')).toEqual([]);
  });
});

describe('projectMatchReasons', () => {
  const ctx = context({
    branches: {
      p1: [branch({ branchName: 'add-metrics' })],
      p2: [branch({ projectId: 'p2', branchName: 'metrics-beta' })],
    },
  });

  it('holds a reason for each shown project matched through an item', () => {
    expect(projectMatchReasons(all, 'metrics', ctx)).toEqual(
      new Map([
        [alpha.id, { kind: 'branch', label: 'add-metrics', more: 0 }],
        [beta.id, { kind: 'branch', label: 'metrics-beta', more: 0 }],
      ])
    );
  });

  it('leaves out projects matched by identity alone', () => {
    // beta matches "beta" by name; its branch matching too doesn't add a reason.
    expect(projectMatchReasons(all, 'beta', ctx).size).toBe(0);
  });

  it('only covers the projects passed in', () => {
    expect([...projectMatchReasons([beta], 'metrics', ctx).keys()]).toEqual([beta.id]);
  });

  it('is the same empty map for every blank query', () => {
    expect(projectMatchReasons(all, '  ', ctx).size).toBe(0);
    expect(projectMatchReasons(all, '', context())).toBe(projectMatchReasons(all, '  ', ctx));
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

// The ⌘1–9 overlay numbers: the grid renders hidden cards too, so they count
// the cards left on screen, not the rendered list.
describe('projectShortcutNumbers', () => {
  it('numbers the shown cards from 1 in grid order', () => {
    expect(projectShortcutNumbers(all)).toEqual(
      new Map([
        [alpha.id, 1],
        [beta.id, 2],
        [gamma.id, 3],
      ])
    );
  });

  it('skips cards the query hid, so the numbers match what ⌘N opens', () => {
    // alpha, first in the grid, is hidden: beta is the first card on screen.
    const shown = visibleProjects(all, all, null, 'b');
    expect(shown).toEqual([beta, gamma]);
    expect(projectShortcutNumbers(shown)).toEqual(
      new Map([
        [beta.id, 1],
        [gamma.id, 2],
      ])
    );
  });

  it('numbers only the first nine', () => {
    const many = Array.from({ length: 12 }, (_, i) => project({ id: `p${i}`, name: `P${i}` }));
    const numbers = projectShortcutNumbers(many);
    expect(numbers.size).toBe(9);
    expect(numbers.get('p8')).toBe(9);
    expect(numbers.has('p9')).toBe(false);
  });

  it('is empty when nothing is shown', () => {
    expect(projectShortcutNumbers([]).size).toBe(0);
  });
});
