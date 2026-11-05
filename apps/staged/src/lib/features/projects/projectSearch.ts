/**
 * Client-side text search for the landing-page project grid and the sidebar.
 *
 * Plain module (no runes, no store imports) so the matching rule stays
 * trivially unit-testable; the caller hands in what to search beyond the
 * project itself as a SearchContext. Applied after the chip filters in
 * ProjectsList.
 *
 * `visibleProjects` is the whole composition the grid shows — chip filters,
 * sticky return-target card, then the query — lifted out of the component so
 * the wiring between the three is testable without a Svelte runtime. The grid
 * renders `withStickyProject` and hides the cards the query rules out, so
 * `visibleProjects` is the cards left on screen, not the ones mounted.
 */

import type { Branch, Project, ProjectRepo } from '../../types';
import { isNumberTerm, queryTerms } from '../../shared/textSearch';
import { projectDisplayName } from '../../shared/utils';

/** Kinds of titled item the backend search index carries. */
export type ProjectSearchIndexKind = 'note' | 'projectNote' | 'review';

/** One titled item of a project in the search index: titles only, no bodies. */
export interface ProjectSearchIndexItem {
  kind: ProjectSearchIndexKind;
  id: string;
  title: string;
  updatedAt: number;
}

/** The search index's items, grouped by project id. */
export type ProjectSearchIndex = ReadonlyMap<string, readonly ProjectSearchIndexItem[]>;

/**
 * What a project is searched by beyond its name, from the shared stores. Each
 * piece is optional data, not a precondition: a project whose branches or
 * repos haven't hydrated yet simply can't match on them until they land.
 */
export interface SearchContext {
  branchesByProject: ReadonlyMap<string, readonly Branch[]>;
  reposByProject: ReadonlyMap<string, readonly ProjectRepo[]>;
  /** A repo's badge short name ("staged"), searched alongside its path. */
  repoShortName?: (githubRepo: string, subpath: string | null) => string | undefined;
  /** Note, project-note and review titles; absent until the index loads. */
  index?: ProjectSearchIndex | null;
}

export type MatchReasonKind = 'branch' | 'pr' | ProjectSearchIndexKind;

/**
 * Why a project matched when its name and repos alone don't explain it: the
 * best matching item and how many others matched besides.
 */
export interface MatchReason {
  kind: MatchReasonKind;
  label: string;
  /** Matching items other than the one shown. */
  more: number;
}

export interface ProjectMatch {
  matched: boolean;
  /** Null when the project matched on its identity (name and repos) alone. */
  reason: MatchReason | null;
}

/** A searchable thing inside a project, with its text already lower-cased. */
interface SearchItem {
  kind: MatchReasonKind;
  label: string;
  text: string[];
  /** Matched exactly by a number term, never as text: `12` must not find PR 123. */
  prNumber: number | null;
  updatedAt: number;
}

/** A query term, with the PR number it stands for when it reads as one. */
interface Term {
  text: string;
  number: number | null;
}

// Which item explains a match best, lowest first: branches and PRs, then
// branch notes, project notes and reviews. Ties go to the most recent.
const KIND_PRIORITY: Record<MatchReasonKind, number> = {
  branch: 0,
  pr: 0,
  note: 1,
  projectNote: 2,
  review: 3,
};

const NO_MATCH: ProjectMatch = { matched: false, reason: null };
const IDENTITY_MATCH: ProjectMatch = { matched: true, reason: null };
const NO_REASONS: ReadonlyMap<string, MatchReason> = new Map();

function parseTerms(terms: readonly string[]): Term[] {
  return terms.map((text) => ({
    text,
    number: isNumberTerm(text) ? Number(text.replace('#', '')) : null,
  }));
}

function repoPath(githubRepo: string, subpath: string | null): string {
  return subpath ? `${githubRepo}/${subpath}` : githubRepo;
}

/**
 * The project's identity, lower-cased: its name plus a label for every repo —
 * the path (and a fork PR's head repo) and the badge short name. Before the
 * project's repos hydrate, the primary repo stands in, as on the grid card.
 */
function identityFields(project: Project, ctx: SearchContext | undefined): string[] {
  const fields = [projectDisplayName(project)];
  if (ctx) {
    const addRepo = (githubRepo: string, subpath: string | null) => {
      fields.push(repoPath(githubRepo, subpath));
      const shortName = ctx.repoShortName?.(githubRepo, subpath);
      if (shortName) fields.push(shortName);
    };
    const repos = ctx.reposByProject.get(project.id) ?? [];
    for (const r of repos) {
      addRepo(r.githubRepo, r.subpath);
      if (r.headRepo) fields.push(repoPath(r.headRepo, r.subpath));
    }
    if (repos.length === 0 && project.githubRepo) addRepo(project.githubRepo, project.subpath);
  }
  return fields.map((f) => f.toLowerCase());
}

/**
 * The project's items: one per branch (its name and PR number together, so
 * a query can't pair one branch's name with another's PR), then whatever the
 * index holds for it.
 */
function projectItems(projectId: string, ctx: SearchContext): SearchItem[] {
  const items: SearchItem[] = [];
  for (const b of ctx.branchesByProject.get(projectId) ?? []) {
    const hasPr = b.prNumber !== null;
    items.push({
      kind: hasPr ? 'pr' : 'branch',
      label: hasPr ? `#${b.prNumber} · ${b.branchName}` : b.branchName,
      text: [b.branchName.toLowerCase()],
      prNumber: b.prNumber,
      updatedAt: b.updatedAt,
    });
  }
  for (const item of ctx.index?.get(projectId) ?? []) {
    items.push({
      kind: item.kind,
      label: item.title,
      text: [item.title.toLowerCase()],
      prNumber: null,
      updatedAt: item.updatedAt,
    });
  }
  return items;
}

function itemHasTerm(item: SearchItem, term: Term): boolean {
  if (term.number !== null && item.prNumber === term.number) return true;
  return item.text.some((t) => t.includes(term.text));
}

function outranks(a: SearchItem, b: SearchItem): boolean {
  const byKind = KIND_PRIORITY[a.kind] - KIND_PRIORITY[b.kind];
  return byKind !== 0 ? byKind < 0 : a.updatedAt > b.updatedAt;
}

/**
 * Whether `project` matches the query `terms` (queryTerms' output), and why.
 *
 * Every term must land somewhere, case-insensitively and in any order, but
 * not just anywhere: a project matches when its identity — name and repo
 * labels — holds every term, or when its identity together with one single
 * item does. So "staged login" finds a project in the staged repo with a
 * branch named fix-login, while "fix login" doesn't match a project just
 * because one branch says "fix" and another says "login". A number term
 * (`123`, `#123`) also matches a branch whose PR is exactly that number.
 *
 * Matching on identity alone gives no reason: the name or repo badge on
 * screen already explains it. Otherwise the reason is the best matching item
 * by kind (KIND_PRIORITY), then recency, counting the rest as `more`.
 *
 * Without a context only the name is searched.
 */
export function matchProject(
  project: Project,
  terms: readonly string[],
  ctx?: SearchContext
): ProjectMatch {
  const identity = identityFields(project, ctx);
  const missing = parseTerms(terms).filter((t) => !identity.some((f) => f.includes(t.text)));
  if (missing.length === 0) return IDENTITY_MATCH;
  if (!ctx) return NO_MATCH;

  let best: SearchItem | null = null;
  let count = 0;
  for (const item of projectItems(project.id, ctx)) {
    if (!missing.every((t) => itemHasTerm(item, t))) continue;
    count++;
    if (!best || outranks(item, best)) best = item;
  }
  if (!best) return NO_MATCH;
  return { matched: true, reason: { kind: best.kind, label: best.label, more: count - 1 } };
}

/**
 * The projects the query keeps, by matchProject's rule, in input order. An
 * empty (or whitespace-only) query matches every project and returns the
 * input array unchanged.
 */
export function searchProjects(projects: Project[], query: string, ctx?: SearchContext): Project[] {
  const terms = queryTerms(query);
  if (terms.length === 0) return projects;
  return projects.filter((p) => matchProject(p, terms, ctx).matched);
}

/**
 * The match reason of each project in `shown` that has one, by project id.
 * Callers pass only the projects on screen, so a keystroke ranks items for
 * the results alone. A blank query always gets the same empty map, so a
 * `$derived` over it stays put while branches update and nobody is searching.
 */
export function projectMatchReasons(
  shown: Project[],
  query: string,
  ctx: SearchContext
): ReadonlyMap<string, MatchReason> {
  const terms = queryTerms(query);
  if (terms.length === 0) return NO_REASONS;
  const reasons = new Map<string, MatchReason>();
  for (const p of shown) {
    const { reason } = matchProject(p, terms, ctx);
    if (reason) reasons.set(p.id, reason);
  }
  return reasons;
}

/**
 * The chip-filter result with the sticky return-target card re-inserted: what
 * the grid shows before the search narrows it.
 *
 * `chipFiltered` is the chip-filter result. When `stickyId` names a project the
 * chips excluded, it is re-inserted — rebuilt from the full `projects` list
 * rather than appended so the card keeps its place in the grid, the position
 * the scroll restore captured. With no re-insertion, `chipFiltered` is
 * returned unchanged.
 */
export function withStickyProject(
  projects: Project[],
  chipFiltered: Project[],
  stickyId: string | null
): Project[] {
  if (!stickyId || chipFiltered.some((p) => p.id === stickyId)) return chipFiltered;
  const matched = new Set(chipFiltered.map((p) => p.id));
  return projects.filter((p) => matched.has(p.id) || p.id === stickyId);
}

/**
 * The projects the landing-page grid shows, in grid order.
 *
 * The query is applied to whatever the chips (and the sticky exception) left.
 * The sticky card is not exempt from the search: the exception exists so a
 * card the user just visited doesn't vanish because visiting it changed its
 * read state, whereas a typed query is a deliberate narrowing, so a sticky
 * card that doesn't match it is hidden.
 *
 * With no sticky re-insertion and an empty query, `chipFiltered` is returned
 * unchanged.
 */
export function visibleProjects(
  projects: Project[],
  chipFiltered: Project[],
  stickyId: string | null,
  query: string,
  ctx?: SearchContext
): Project[] {
  return searchProjects(withStickyProject(projects, chipFiltered, stickyId), query, ctx);
}

/**
 * The ⌘1–9 number of each card on screen, by project id: the first nine of
 * `shown` (visibleProjects' result) in grid order. The grid keeps cards the
 * query hides mounted, so a card's place in the rendered list is not its
 * place on screen; ⌘N opens `shown[N - 1]`, and these are the numbers the
 * overlay has to show for that.
 */
export function projectShortcutNumbers(shown: Project[]): Map<string, number> {
  return new Map(shown.slice(0, 9).map((p, i) => [p.id, i + 1]));
}
