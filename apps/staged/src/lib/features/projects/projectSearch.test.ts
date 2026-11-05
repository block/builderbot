import { describe, expect, it } from 'vitest';
import type { Branch, Project, ProjectRepo } from '../../types';
import { queryTerms } from '../../shared/textSearch';
import { labelTerms, matchLabelSegments } from './matchReasonLabel';
import {
  buildSearchSnapshot,
  projectShortcutNumbers,
  searchProjects,
  visibleProjects,
  withStickyProject,
  type ProjectSearchIndexItem,
  type SearchContext,
} from './projectSearch';

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
  it('returns the same array, with no rank, for an empty query', () => {
    expect(searchProjects(all, '').projects).toBe(all);
    expect(searchProjects(all, '').rank).toBeNull();
  });

  it('treats a whitespace-only query as empty', () => {
    expect(searchProjects(all, '   ').projects).toBe(all);
  });

  it('matches by substring of the name, case-insensitively', () => {
    expect(searchProjects(all, 'ALPHA').projects).toEqual([alpha]);
    expect(searchProjects(all, 'tool').projects).toEqual([beta]);
    expect(searchProjects(all, 'a').projects).toEqual([alpha, beta, gamma]);
  });

  it('trims whitespace around the query before matching', () => {
    expect(searchProjects(all, '  beta ').projects).toEqual([beta]);
  });

  it('matches every whitespace-separated term, in any order', () => {
    const oneTwoThree = project({ id: 'p4', name: 'one two three' });
    const list = [...all, oneTwoThree];
    expect(searchProjects(list, 'one three').projects).toEqual([oneTwoThree]);
    expect(searchProjects(list, 'three one').projects).toEqual([oneTwoThree]);
    expect(searchProjects(list, 'one four').projects).toEqual([]);
    expect(searchProjects(list, 'two').projects).toEqual([oneTwoThree]);
  });

  it('is case-insensitive across every term', () => {
    const oneTwoThree = project({ id: 'p4', name: 'One Two THREE' });
    expect(searchProjects([...all, oneTwoThree], 'oNe tHree').projects).toEqual([oneTwoThree]);
  });

  it('ignores how the query and name space their words', () => {
    expect(searchProjects(all, 'gamma  web').projects).toEqual([gamma]);
    expect(searchProjects(all, 'gamma web').projects).toEqual([gamma]);
  });

  it('returns an empty list when nothing matches', () => {
    expect(searchProjects(all, 'zeta').projects).toEqual([]);
  });

  it('does not match on the repo slug without a context', () => {
    expect(searchProjects(all, 'org/').projects).toEqual([]);
  });

  it('keeps input order among equal scores', () => {
    expect(searchProjects([gamma, alpha, beta], 'a').projects).toEqual([gamma, alpha, beta]);
  });

  it("ranks each kept project by its place in the result, for the surfaces' CSS order", () => {
    expect(searchProjects([gamma, alpha, beta], 'a').rank).toEqual(
      new Map([
        [gamma.id, 0],
        [alpha.id, 1],
        [beta.id, 2],
      ])
    );
    expect(searchProjects(all, 'tool').rank).toEqual(new Map([[beta.id, 0]]));
    expect(searchProjects(all, 'zeta').rank).toEqual(new Map());
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
    prTitle: null,
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

function indexItem(
  overrides: Partial<Omit<ProjectSearchIndexItem, 'text'>> = {}
): ProjectSearchIndexItem {
  const item = { kind: 'note' as const, id: 'n1', title: 'A note', updatedAt: 0, ...overrides };
  return { ...item, text: item.title.toLowerCase() };
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

/** Whether `p` matches `query`, and why, through searchProjects. */
function match(p: Project, query: string, ctx?: SearchContext) {
  const { projects, reasons } = searchProjects([p], query, ctx && buildSearchSnapshot([p], ctx));
  return { matched: projects.length === 1, reason: reasons.get(p.id) ?? null };
}

describe('the matching rule', () => {
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
        reason: { kind: 'branch', label: 'tidy-typos', more: 0, itemTerms: ['typos'] },
      });
    });

    it('labels a branch with a PR by its number and, before the title lands, its name', () => {
      expect(match(app, 'redirect', ctx).reason).toEqual({
        kind: 'pr',
        label: 'PR #123 · fix-login-redirect',
        more: 0,
        itemTerms: ['redirect'],
      });
    });

    it('matches `#123` and `123` to PR 123 exactly', () => {
      expect(match(app, '#123', ctx).reason?.label).toBe('PR #123 · fix-login-redirect');
      expect(match(app, '123', ctx).reason?.label).toBe('PR #123 · fix-login-redirect');
    });

    it('does not match a prefix of a PR number', () => {
      expect(match(app, '12', ctx).matched).toBe(false);
      expect(match(app, '#12', ctx).matched).toBe(false);
    });

    it('still matches a number term as text', () => {
      expect(match(app, '456', ctx).reason?.label).toBe('issue-4567-crash');
      // No PR explains it, so it is the item's own match.
      expect(match(app, '456', ctx).reason?.itemTerms).toEqual(['456']);
    });

    it('lets the terms spread across branches', () => {
      expect(match(app, 'login 123', ctx).matched).toBe(true);
      // "login" is one branch, "typos" another: both contributed, and the
      // more recent shows.
      expect(match(app, 'login typos', ctx)).toEqual({
        matched: true,
        reason: { kind: 'branch', label: 'tidy-typos', more: 1, itemTerms: ['typos'] },
      });
      // PR 123 is fix-login-redirect's, "typos" tidy-typos': the PR outranks
      // the branch name.
      expect(match(app, 'typos #123', ctx)).toEqual({
        matched: true,
        reason: { kind: 'pr', label: 'PR #123 · fix-login-redirect', more: 1, itemTerms: [] },
      });
      // Every term still has to land somewhere.
      expect(match(app, 'login nope', ctx).matched).toBe(false);
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
        itemTerms: ['-'],
      });
    });

    it('cannot match on branches that have not hydrated', () => {
      expect(match(app, 'typos', context()).matched).toBe(false);
    });
  });

  describe('PR queries written the way the label reads', () => {
    const ctx = context({
      branches: {
        p1: [
          branch({ id: 'b1', branchName: 'fix-login-redirect', prNumber: 123, updatedAt: 1 }),
          branch({ id: 'b2', branchName: 'tidy-typos', updatedAt: 2 }),
          branch({ id: 'b3', branchName: 'add-metrics', prNumber: 77, updatedAt: 3 }),
        ],
      },
    });

    it('matches `pr123`, `pr#123`, `PR #123` and `pr 123` to PR 123', () => {
      for (const query of ['pr123', 'pr#123', 'PR#123', 'PR #123', 'pr 123', 'Pr 123']) {
        // The `PR #123` prefix explains every term, so none moves the window.
        expect(match(app, query, ctx).reason, query).toEqual({
          kind: 'pr',
          label: 'PR #123 · fix-login-redirect',
          more: 0,
          itemTerms: [],
        });
      }
    });

    it('still requires the exact number', () => {
      for (const query of ['pr12', 'pr#12', 'PR #12', 'pr 12', 'pr 1234']) {
        expect(match(app, query, ctx).matched, query).toBe(false);
      }
    });

    it('lets a bare `pr` match any branch with a PR, not one without', () => {
      expect(match(app, 'pr', ctx).reason).toEqual({
        kind: 'pr',
        label: 'PR #77 · add-metrics',
        more: 1,
        itemTerms: [],
      });
      expect(match(app, 'pr login', ctx).reason?.label).toBe('PR #123 · fix-login-redirect');
    });

    it('lets `pr` land on one branch and the other terms on another', () => {
      // tidy-typos has no PR; the project's PRs meet `pr`. Every PR would,
      // so they don't count as contributing next to a term of the item's own.
      expect(match(app, 'pr typos', ctx)).toEqual({
        matched: true,
        reason: { kind: 'branch', label: 'tidy-typos', more: 0, itemTerms: ['typos'] },
      });
    });

    it('cannot match `pr` on a project without a PR', () => {
      const noPrs = context({ branches: { p1: [branch({ branchName: 'tidy-typos' })] } });
      expect(match(app, 'pr', noPrs).matched).toBe(false);
      expect(match(app, 'pr123', noPrs).matched).toBe(false);
    });

    it('still matches `pr` and `pr123` as text', () => {
      const texts = context({
        index: {
          p1: [
            indexItem({ id: 'n1', title: 'Improve caching' }),
            indexItem({ id: 'n2', title: 'Notes on pr123' }),
          ],
        },
      });
      expect(match(app, 'pr', texts).reason?.label).toBe('Improve caching');
      expect(match(app, 'pr123', texts).reason?.label).toBe('Notes on pr123');
      // Matched in the text, not through a PR, so they still window the label.
      expect(match(app, 'pr', texts).reason?.itemTerms).toEqual(['pr']);
      expect(match(app, 'pr123', texts).reason?.itemTerms).toEqual(['pr123']);
    });
  });

  describe('PR titles', () => {
    const ctx = context({
      branches: {
        p1: [
          branch({
            id: 'b1',
            branchName: 'mt/auth-fix',
            prNumber: 123,
            prTitle: 'Fix login redirect',
            updatedAt: 1,
          }),
          branch({ id: 'b2', branchName: 'tidy-typos', updatedAt: 2 }),
        ],
      },
    });

    it('matches the title, labelled `PR #123 · <title>`', () => {
      expect(match(app, 'redirect', ctx)).toEqual({
        matched: true,
        reason: {
          kind: 'pr',
          label: 'PR #123 · Fix login redirect',
          more: 0,
          itemTerms: ['redirect'],
        },
      });
    });

    it('labels by the branch name when only it holds the terms, so the match shows', () => {
      expect(match(app, 'auth-fix', ctx).reason).toEqual({
        kind: 'pr',
        label: 'PR #123 · mt/auth-fix',
        more: 0,
        itemTerms: ['auth-fix'],
      });
      // The title holds one of the terms: the title wins.
      expect(match(app, 'auth-fix login', ctx).reason?.label).toBe('PR #123 · Fix login redirect');
    });

    it('labels by the title when only the number matched', () => {
      expect(match(app, '#123', ctx).reason?.label).toBe('PR #123 · Fix login redirect');
    });

    it('lists as itemTerms only what the item matched beyond the name and its PR', () => {
      // "checkout" is the project name's; "pr" and "#123" the PR prefix's.
      expect(match(app, 'checkout pr #123 redirect', ctx).reason?.itemTerms).toEqual(['redirect']);
      expect(match(app, 'pr redirect', ctx).reason?.itemTerms).toEqual(['redirect']);
      expect(match(app, 'pr123 login', ctx).reason?.itemTerms).toEqual(['login']);
    });

    it('windows a long title on `pr redirect` as ProjectMatchReason renders it', () => {
      const long = context({
        branches: {
          p1: [
            branch({
              branchName: 'mt/auth-fix',
              prNumber: 123,
              prTitle: 'Fix the very long and winding redirect handling for the login page',
            }),
          ],
        },
      });
      const query = 'pr redirect';
      const reason = match(app, query, long).reason!;
      const terms = queryTerms(query);
      const shown = matchLabelSegments(
        reason.label,
        labelTerms(terms),
        labelTerms(reason.itemTerms)
      )
        .map((s) => (s.match ? `[${s.text}]` : s.text))
        .join('');
      expect(shown).toBe('…and winding [redirect] handling for the login page');
    });

    it('pairs the title with another branch, the PR showing', () => {
      expect(match(app, 'login typos', ctx).reason).toEqual({
        kind: 'pr',
        label: 'PR #123 · Fix login redirect',
        more: 1,
        itemTerms: ['login'],
      });
    });

    it('ignores a title left on a branch without a PR', () => {
      const unlinked = context({
        branches: { p1: [branch({ branchName: 'mt/auth-fix', prTitle: 'Fix login redirect' })] },
      });
      expect(match(app, 'redirect', unlinked).matched).toBe(false);
      expect(match(app, 'auth', unlinked).reason).toEqual({
        kind: 'branch',
        label: 'mt/auth-fix',
        more: 0,
        itemTerms: ['auth'],
      });
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

    it('shows the best field that contributed: branches, project notes, notes, reviews', () => {
      // The branch holds "login" at a branch name's weight; the titles that
      // also say it don't contribute, so they aren't counted.
      expect(match(app, 'login', ctx).reason).toEqual({
        kind: 'branch',
        label: 'login-page',
        more: 0,
        itemTerms: ['login'],
      });
      expect(match(app, 'plan', ctx).reason?.kind).toBe('projectNote');
      expect(match(app, 'flow', ctx).reason?.kind).toBe('review');
    });

    it('ranks project notes over notes over reviews', () => {
      const titles = (items: ProjectSearchIndexItem[]) => context({ index: { p1: items } });
      const review = indexItem({ kind: 'review', id: 'v1', title: 'Review login', updatedAt: 9 });
      const note = indexItem({ kind: 'note', id: 'n1', title: 'Login notes', updatedAt: 5 });
      const plan = indexItem({ kind: 'projectNote', id: 'pn1', title: 'Login plan' });
      expect(match(app, 'login', titles([review, note, plan])).reason?.label).toBe('Login plan');
      expect(match(app, 'login', titles([review, note])).reason?.label).toBe('Login notes');
      expect(match(app, 'login', titles([review])).reason?.label).toBe('Review login');
    });

    it('counts every item that contributed at the best weight', () => {
      const notes = context({
        index: {
          p1: [
            indexItem({ id: 'n1', title: 'Login notes', updatedAt: 2 }),
            indexItem({ id: 'n2', title: 'Login research', updatedAt: 3 }),
            indexItem({ kind: 'review', id: 'v1', title: 'Review login' }),
          ],
        },
      });
      // Both notes; the review is outranked.
      expect(match(app, 'login', notes).reason).toEqual({
        kind: 'note',
        label: 'Login research',
        more: 1,
        itemTerms: ['login'],
      });
    });

    it('prefers an item holding every term, then the most recent', () => {
      // "login" contributes through the branch, "es" through the notes "Login
      // notes" and "Login research", which hold both terms and earn the
      // one-item bonus: the more recent of the two shows.
      expect(match(app, 'login es', ctx).reason).toEqual({
        kind: 'note',
        label: 'Login research',
        more: 2,
        itemTerms: ['login', 'es'],
      });
    });

    it('pairs terms from two different items', () => {
      // "redirect" is in n3, "plan" in pn1; the project note outranks the note.
      expect(match(app, 'redirect plan', ctx)).toEqual({
        matched: true,
        reason: { kind: 'projectNote', label: 'Login plan', more: 1, itemTerms: ['plan'] },
      });
    });

    it('matches a number term against titles as text', () => {
      expect(match(app, '#123', ctx).reason?.label).toBe('PR #123: Fix login');
    });
  });
});

describe('searchProjects with a snapshot', () => {
  const snapshot = buildSearchSnapshot(
    all,
    context({
      branches: {
        p1: [branch({ branchName: 'add-metrics', prNumber: 42 })],
        p3: [branch({ projectId: 'p3', branchName: 'metrics-dashboard' })],
      },
    })
  );

  it('keeps the input order for equal scores, whatever each project matched on', () => {
    expect(searchProjects([gamma, beta, alpha], 'metrics', snapshot).projects).toEqual([
      gamma,
      alpha,
    ]);
    expect(searchProjects(all, 'metrics', snapshot).projects).toEqual([alpha, gamma]);
  });

  it('still matches names', () => {
    expect(searchProjects(all, 'beta', snapshot).projects).toEqual([beta]);
  });

  it('is applied by visibleProjects', () => {
    expect(visibleProjects(all, all, null, '#42', snapshot).projects).toEqual([alpha]);
    expect(visibleProjects(all, all, null, '#42').projects).toEqual([]);
  });

  it('matches a project missing from the snapshot on its name alone', () => {
    const late = project({ id: 'p9', name: 'Metrics late' });
    expect(searchProjects([late], 'metrics', snapshot).projects).toEqual([late]);
    expect(searchProjects([late], 'org/alpha', snapshot).projects).toEqual([]);
  });
});

describe('searchProjects reasons', () => {
  const snapshot = buildSearchSnapshot(
    all,
    context({
      branches: {
        p1: [branch({ branchName: 'add-metrics' })],
        p2: [branch({ projectId: 'p2', branchName: 'metrics-beta' })],
      },
    })
  );

  it('holds a reason for each kept project matched through an item', () => {
    expect(searchProjects(all, 'metrics', snapshot).reasons).toEqual(
      new Map([
        [alpha.id, { kind: 'branch', label: 'add-metrics', more: 0, itemTerms: ['metrics'] }],
        [beta.id, { kind: 'branch', label: 'metrics-beta', more: 0, itemTerms: ['metrics'] }],
      ])
    );
  });

  it('leaves out projects matched by identity alone', () => {
    // beta matches "beta" by name; its branch matching too doesn't add a reason.
    expect(searchProjects(all, 'beta', snapshot).reasons.size).toBe(0);
  });

  it('only covers the projects passed in', () => {
    expect([...searchProjects([beta], 'metrics', snapshot).reasons.keys()]).toEqual([beta.id]);
  });

  it('is the same empty map for every blank query', () => {
    const blank = searchProjects(all, '  ', snapshot).reasons;
    expect(blank.size).toBe(0);
    expect(searchProjects(all, '').reasons).toBe(blank);
  });
});

// ── Ranking ──

describe('searchProjects ranking', () => {
  /** The ids searchProjects keeps, best first. */
  const ranked = (list: Project[], query: string, ctx: SearchContext) =>
    searchProjects(list, query, buildSearchSnapshot(list, ctx)).projects.map((p) => p.id);

  // One project per field, each holding "zz" only there.
  const inName = project({ id: 'name', name: 'Has zz' });
  const inPr = project({ id: 'pr', name: 'P pr' });
  const inBranch = project({ id: 'branch', name: 'P branch' });
  const inRepo = project({ id: 'repo', name: 'P repo', githubRepo: 'org/zz-repo' });
  const inProjectNote = project({ id: 'projectNote', name: 'P project note' });
  const inNote = project({ id: 'note', name: 'P note' });
  const inReview = project({ id: 'review', name: 'P review' });
  const tiers = context({
    branches: {
      pr: [branch({ projectId: 'pr', branchName: 'b-pr', prNumber: 5, prTitle: 'Add zz' })],
      branch: [branch({ projectId: 'branch', branchName: 'zz-branch' })],
    },
    index: {
      projectNote: [indexItem({ kind: 'projectNote', id: 'pn', title: 'zz plan' })],
      note: [indexItem({ kind: 'note', id: 'n', title: 'zz notes' })],
      review: [indexItem({ kind: 'review', id: 'v', title: 'zz review' })],
    },
  });
  const tierOrder = ['name', 'pr', 'branch', 'repo', 'projectNote', 'note', 'review'];

  it('orders by the best field a term hit: name, PR, branch, repo, project note, note, review', () => {
    const shuffled = [inReview, inRepo, inNote, inName, inProjectNote, inBranch, inPr];
    expect(ranked(shuffled, 'zz', tiers)).toEqual(tierOrder);
    expect(ranked([...shuffled].reverse(), 'zz', tiers)).toEqual(tierOrder);
  });

  it('ranks a PR number with PR titles, above branch names', () => {
    const byNumber = project({ id: 'number', name: 'By number' });
    const byBranch = project({ id: 'branch', name: 'By branch' });
    const ctx = context({
      branches: {
        number: [branch({ projectId: 'number', branchName: 'tidy', prNumber: 77 })],
        branch: [branch({ projectId: 'branch', branchName: 'issue-77' })],
      },
    });
    expect(ranked([byBranch, byNumber], '77', ctx)).toEqual(['number', 'branch']);
    expect(ranked([byBranch, byNumber], '#77', ctx)).toEqual(['number']);
  });

  it('sums the terms, each at the best field it was found in', () => {
    // Name + note (64 + 2) beats PR + PR on two branches (32 + 32).
    const nameAndNote = project({ id: 'a', name: 'Fix stuff' });
    const twoPrs = project({ id: 'b', name: 'Other' });
    const ctx = context({
      branches: {
        b: [
          branch({ id: 'b1', projectId: 'b', prNumber: 1, prTitle: 'Fix flaky test' }),
          branch({ id: 'b2', projectId: 'b', prNumber: 2, prTitle: 'Login form' }),
        ],
      },
      index: { a: [indexItem({ title: 'Login research' })] },
    });
    expect(ranked([twoPrs, nameAndNote], 'fix login', ctx)).toEqual(['a', 'b']);
  });

  it('puts every term in the name above the same terms split with an item', () => {
    // "fix" in the name and "login fix" in a PR title score 64 + 32 + the
    // one-item bonus, which only the all-in-name bonus outranks.
    const allInName = project({ id: 'a', name: 'Fix login' });
    const split = project({ id: 'b', name: 'Fix stuff' });
    const ctx = context({
      branches: { b: [branch({ projectId: 'b', prNumber: 1, prTitle: 'Login fix' })] },
    });
    expect(ranked([split, allInName], 'fix login', ctx)).toEqual(['a', 'b']);
  });

  it('puts every term in one item above the same terms split across items', () => {
    // One note saying both beats a PR with one and a note with the other.
    const oneNote = project({ id: 'a', name: 'One' });
    const split = project({ id: 'b', name: 'Two' });
    const ctx = context({
      branches: { b: [branch({ projectId: 'b', prNumber: 1, prTitle: 'Fix flaky test' })] },
      index: {
        a: [indexItem({ title: 'Fix login redirect' })],
        b: [indexItem({ title: 'Login research' })],
      },
    });
    expect(ranked([split, oneNote], 'fix login', ctx)).toEqual(['a', 'b']);
  });

  it("counts a branch's name, PR number and PR title as one item", () => {
    const oneBranch = project({ id: 'a', name: 'One' });
    const twoBranches = project({ id: 'b', name: 'Two' });
    const ctx = context({
      branches: {
        a: [branch({ projectId: 'a', branchName: 'mt/auth', prNumber: 9, prTitle: 'Login' })],
        b: [
          branch({ id: 'b1', projectId: 'b', branchName: 'mt/auth' }),
          branch({ id: 'b2', projectId: 'b', branchName: 'x', prNumber: 8, prTitle: 'Login' }),
        ],
      },
    });
    expect(ranked([twoBranches, oneBranch], 'auth login', ctx)).toEqual(['a', 'b']);
  });

  it('gives a one-word query no one-item bonus, so it orders by field alone', () => {
    // Were the bonus given, the note would outrank the repo label.
    expect(ranked([inNote, inRepo], 'zz', tiers)).toEqual(['repo', 'note']);
  });

  it('keeps the unsearched order among equal scores', () => {
    const [a, b, c, d] = ['a', 'b', 'c', 'd'].map((id) => project({ id, name: id.toUpperCase() }));
    const ctx = context({
      branches: {
        a: [branch({ projectId: 'a', branchName: 'x-metrics' })],
        b: [branch({ projectId: 'b', branchName: 'y-metrics' })],
        d: [branch({ projectId: 'd', branchName: 'z-metrics' })],
      },
      index: { c: [indexItem({ title: 'Metrics plan' })] },
    });
    expect(ranked([d, c, b, a], 'metrics', ctx)).toEqual(['d', 'b', 'a', 'c']);
    expect(ranked([a, b, c, d], 'metrics', ctx)).toEqual(['a', 'b', 'd', 'c']);
  });

  it('ranks the reasons the same pass gives', () => {
    const list = [inReview, inName, inPr];
    const result = searchProjects(list, 'zz', buildSearchSnapshot(list, tiers));
    expect(result.rank).toEqual(
      new Map([
        ['name', 0],
        ['pr', 1],
        ['review', 2],
      ])
    );
    expect(result.reasons.has('name')).toBe(false);
    expect(result.reasons.get('pr')?.label).toBe('PR #5 · Add zz');
    expect(result.reasons.get('review')?.label).toBe('zz review');
  });

  it('leaves an empty query exactly as it was', () => {
    const list = [inReview, inName, inPr];
    const result = searchProjects(list, ' ', buildSearchSnapshot(list, tiers));
    expect(result.projects).toBe(list);
    expect(result.rank).toBeNull();
    expect(result.reasons.size).toBe(0);
  });
});

describe('buildSearchSnapshot', () => {
  const linked = branch({ id: 'b1', branchName: 'fix-login', prNumber: 123, prTitle: 'Fix it' });
  const repos = { p1: [projectRepo()] };
  const notes = { p1: [indexItem({ id: 'n1', title: 'Login research' })] };
  const base = () =>
    context({ branches: { p1: [linked] }, repos, index: notes, shortNames: { 'org/alpha': 'al' } });

  it('hands back the previous snapshot when a PR status poll moved nothing it reads', () => {
    const first = buildSearchSnapshot(all, base());
    // A poll flush: a new map and list, a new branch object, new status fields.
    const polled = base();
    polled.branchesByProject = new Map([
      ['p1', [{ ...linked, prState: 'MERGED', prChecksStatus: 'SUCCESS', prFetchedAt: 9 }]],
    ]);
    expect(buildSearchSnapshot(all, polled, first)).toBe(first);
    // Same lists again: the fast path.
    expect(buildSearchSnapshot(all, base(), first)).toBe(first);
  });

  it('hands back the previous snapshot when the index refetches the same titles', () => {
    const first = buildSearchSnapshot(all, base());
    const refetched = base();
    refetched.index = new Map([['p1', [indexItem({ id: 'n1', title: 'Login research' })]]]);
    expect(buildSearchSnapshot(all, refetched, first)).toBe(first);
  });

  it('builds a new snapshot when something a search reads changed', () => {
    const first = buildSearchSnapshot(all, base());
    const changes: Array<[string, Partial<SearchContext>]> = [
      ['branch name', { branchesByProject: branchesWith({ branchName: 'fix-auth' }) }],
      ['PR number', { branchesByProject: branchesWith({ prNumber: 124 }) }],
      ['PR title', { branchesByProject: branchesWith({ prTitle: 'Fix it again' }) }],
      ['recency', { branchesByProject: branchesWith({ updatedAt: 5 }) }],
      ['a new branch', { branchesByProject: new Map([['p1', [linked, branch()]]]) }],
      [
        'an index title',
        { index: new Map([['p1', [indexItem({ id: 'n1', title: 'Login plan' })]]]) },
      ],
      ['a badge short name', { repoShortName: () => 'alp' }],
      ['a repo', { reposByProject: new Map([['p1', [projectRepo({ headRepo: 'fork/x' })]]]) }],
    ];
    for (const [what, change] of changes) {
      expect(buildSearchSnapshot(all, { ...base(), ...change }, first), what).not.toBe(first);
    }
  });

  it('builds a new snapshot when a project is renamed, added or removed', () => {
    const first = buildSearchSnapshot(all, base());
    const renamed = [project({ id: 'p1', name: 'Renamed' }), beta, gamma];
    expect(buildSearchSnapshot(renamed, base(), first)).not.toBe(first);
    expect(buildSearchSnapshot([alpha, beta], base(), first)).not.toBe(first);
    expect(buildSearchSnapshot([...all, project({ id: 'p4' })], base(), first)).not.toBe(first);
  });

  it('matches by the fresh data after a rebuild', () => {
    const first = buildSearchSnapshot(all, base());
    const ctx = base();
    ctx.branchesByProject = branchesWith({ prTitle: 'Speed up search' });
    const next = buildSearchSnapshot(all, ctx, first);
    expect(searchProjects(all, 'speed', first).projects).toEqual([]);
    expect(searchProjects(all, 'speed', next).projects).toEqual([alpha]);
  });

  function branchesWith(overrides: Partial<Branch>) {
    return new Map([['p1', [{ ...linked, ...overrides }]]]);
  }
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
    expect(visibleProjects(all, [], beta.id, 'alpha').projects).toEqual([]);
  });
});

// The grid composition: chip filters → sticky return-target card → query.
describe('visibleProjects', () => {
  describe('without a sticky card', () => {
    it('returns the chip-filtered list unchanged for an empty query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, null, '').projects).toBe(chipsWithoutBeta);
    });

    it('applies the query to the chip-filtered list', () => {
      expect(visibleProjects(all, chipsWithoutBeta, null, 'gamma').projects).toEqual([gamma]);
      // beta matches the query but the chips excluded it.
      expect(visibleProjects(all, chipsWithoutBeta, null, 'beta').projects).toEqual([]);
    });
  });

  describe('with a sticky card the chips already include', () => {
    it('returns the chip-filtered list unchanged for an empty query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, alpha.id, '').projects).toBe(chipsWithoutBeta);
    });

    it('applies the query to the chip-filtered list', () => {
      expect(visibleProjects(all, chipsWithoutBeta, alpha.id, 'alpha').projects).toEqual([alpha]);
    });

    it('hides the sticky card when it does not match the query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, alpha.id, 'gamma').projects).toEqual([gamma]);
    });
  });

  describe('with a sticky card the chips excluded', () => {
    it('re-inserts it at its position in the full list', () => {
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, '').projects).toEqual([
        alpha,
        beta,
        gamma,
      ]);
    });

    it('re-inserts it and then applies the query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, 'beta').projects).toEqual([beta]);
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, 'a').projects).toEqual([
        alpha,
        beta,
        gamma,
      ]);
    });

    it('hides the sticky card when it does not match the query', () => {
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, 'gamma').projects).toEqual([gamma]);
    });

    it('re-inserts it even when the chips matched nothing', () => {
      expect(visibleProjects(all, [], beta.id, '').projects).toEqual([beta]);
      expect(visibleProjects(all, [], beta.id, 'alpha').projects).toEqual([]);
    });

    it('leaves the list alone when the sticky id is no longer a project', () => {
      // A deleted return target: nothing to re-insert.
      expect(visibleProjects(all, chipsWithoutBeta, 'gone', '').projects).toEqual(chipsWithoutBeta);
    });
  });

  describe('ranking with a sticky card', () => {
    const snapshot = buildSearchSnapshot(
      all,
      context({
        branches: { p1: [branch({ branchName: 'tools-upgrade' })] },
        index: { p2: [indexItem({ title: 'Service migration' })] },
      })
    );

    it('ranks the sticky card like any other, not pinned first', () => {
      // beta (sticky) matches "service" through a note; alpha by its name.
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, 'service', snapshot).projects).toEqual(
        [alpha, beta]
      );
      // beta matches "tools" by its name; alpha through a branch.
      expect(visibleProjects(all, chipsWithoutBeta, beta.id, 'tools', snapshot).projects).toEqual([
        beta,
        alpha,
      ]);
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
    const shown = visibleProjects(all, all, null, 'b').projects;
    expect(shown).toEqual([beta, gamma]);
    expect(projectShortcutNumbers(shown)).toEqual(
      new Map([
        [beta.id, 1],
        [gamma.id, 2],
      ])
    );
  });

  it('follows the ranked order while searching, not the rendered one', () => {
    const snapshot = buildSearchSnapshot(
      all,
      context({ index: { p1: [indexItem({ title: 'Web notes' })] } })
    );
    // alpha renders first but only a note holds "web"; gamma has it in its name.
    const shown = visibleProjects(all, all, null, 'web', snapshot).projects;
    expect(shown).toEqual([gamma, alpha]);
    expect(projectShortcutNumbers(shown)).toEqual(
      new Map([
        [gamma.id, 1],
        [alpha.id, 2],
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
