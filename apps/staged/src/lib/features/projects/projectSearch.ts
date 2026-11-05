/**
 * Client-side text search for the landing-page project grid and the sidebar.
 *
 * Plain module (no runes, no store imports) so the matching and ranking rules
 * stay trivially unit-testable; the caller hands in what to search beyond the
 * project names as a SearchContext. Applied after the chip filters in
 * ProjectsList.
 *
 * The work is split by how often it runs. `buildSearchSnapshot` runs when
 * the data changes: it lower-cases every name, repo label, branch and title
 * once, and hands back the previous snapshot when nothing matching reads has
 * moved, so a PR status poll (fresh branch objects, same names and titles)
 * doesn't re-run a search held on screen. `searchProjects` runs per
 * keystroke: it parses the query once, tests the prepared strings in place,
 * scores each match (FIELD_WEIGHTS), builds a label only for the item each
 * result shows, and returns the ranked matches and their reasons from one
 * pass.
 *
 * Neither surface re-renders in rank order. Both keep every row mounted in
 * its unsearched DOM order, hide what the query rules out, and lay the rest
 * out by `ProjectSearchResult.rank` through CSS `order`, so a keystroke flips
 * attributes and styles on rows that already exist rather than re-keying an
 * `{#each}`.
 *
 * `visibleProjects` is the whole composition the grid shows — chip filters,
 * sticky return-target card, then the query — lifted out of the component so
 * the wiring between the three is testable without a Svelte runtime.
 */

import type { Branch, Project, ProjectRepo, ProjectSearchItemKind } from '../../types';
import { queryTerms, termPrNumber } from '../../shared/textSearch';
import { projectDisplayName } from '../../shared/utils';

// ── Ranking ──
//
// While a query is active the results are ordered by score, highest first;
// equal scores keep the unsearched order. Each term scores the weight of the
// best field it was found in, and a project's score is the sum over its
// terms, plus MATCH_BONUS.allInName when every term is in the project's
// name, or MATCH_BONUS.allInOneItem when a single branch (with its PR), note
// or review holds every term of a multi-word query. The weights are powers
// of two, so a one-word query orders strictly by the best field it hit.

/** What a query term is worth, by the best field it was found in. */
export const FIELD_WEIGHTS = {
  /** The project's name. */
  name: 64,
  /** A branch's PR: its number (exactly), its title, or a bare `pr`. */
  pr: 32,
  /** A branch name. */
  branch: 16,
  /** A repo's path, fork head repo or badge short name. */
  repo: 8,
  /** A project note's title. */
  projectNote: 4,
  /** A branch note's title. */
  note: 2,
  /** A review's title. */
  review: 1,
} as const;

/** Added once to a project's score when every term lands in one place. */
export const MATCH_BONUS = {
  /** Every term is in the project's name. */
  allInName: 64,
  /** One branch, note or review holds every term. Multi-word queries only:
   *  one word is in one item whenever an item holds it at all. */
  allInOneItem: 32,
} as const;

/** One titled item of a project in the search index: titles only, no bodies. */
export interface ProjectSearchIndexItem {
  kind: ProjectSearchItemKind;
  id: string;
  title: string;
  /** The title lower-cased, once when the index loads, not per keystroke. */
  text: string;
  updatedAt: number;
}

/** The search index's items, grouped by project id (projectSearchIndex). */
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

export type MatchReasonKind = 'branch' | 'pr' | ProjectSearchItemKind;

/**
 * Why a project matched when its name and repos alone don't explain it: the
 * best contributing item and how many others contributed besides.
 */
export interface MatchReason {
  kind: MatchReasonKind;
  label: string;
  /** Contributing items other than the one shown. */
  more: number;
  /**
   * The query terms (queryTerms) this item matched for itself: the ones it
   * holds that the name or a repo label doesn't explain better, minus any
   * its PR prefix explains (a bare `pr`, or the PR's number). The label
   * windows on the first of these, so a term that highlights early anyway —
   * in the `PR #123` prefix, or one the name already matched — doesn't hold
   * a long title's match off screen.
   */
  itemTerms: readonly string[];
}

/** A search's result: what it keeps, in what order, and why, from one pass. */
export interface ProjectSearchResult {
  /** The projects the query keeps, best match first and equal scores in
   *  input order; for a blank query, the input array itself. */
  projects: Project[];
  /** Each kept project's index in `projects`, by id: the CSS `order` the
   *  surfaces lay kept rows out by, and the set they hide the rest by. Null
   *  for a blank query, which keeps everything in its own order. */
  rank: ReadonlyMap<string, number> | null;
  /** Why each kept project matched, by id, when its name and repos alone
   *  don't say. */
  reasons: ReadonlyMap<string, MatchReason>;
}

/** A searchable thing inside a project, its text lower-cased in advance. */
interface SearchItem {
  kind: MatchReasonKind;
  /** The branch name or title, as shown. */
  name: string;
  /** The PR's title, on a branch with a PR once a poll or link brings it. */
  prTitle: string | null;
  /** Matched exactly by a number term, never as text: `12` must not find PR 123. */
  prNumber: number | null;
  updatedAt: number;
  /** `name` lower-cased: a branch name or a title. */
  text: string;
  /** What a term found in `text` is worth (FIELD_WEIGHTS). */
  weight: number;
  /** `prTitle` lower-cased, worth FIELD_WEIGHTS.pr. */
  prText: string | null;
}

/** One project's prepared search data. */
export interface SearchSnapshotEntry {
  /** The project's name, lower-cased. */
  readonly name: string;
  /** Its repo labels, lower-cased. */
  readonly repos: readonly string[];
  readonly items: readonly SearchItem[];
  /** The context lists `items` came from. Bookkeeping, updated in place when
   *  an entry is kept, so an unchanged list skips the item-by-item compare. */
  branches: readonly Branch[] | undefined;
  indexItems: readonly ProjectSearchIndexItem[] | undefined;
}

/** Everything a search matches against, by project id (buildSearchSnapshot). */
export interface SearchSnapshot {
  readonly entries: ReadonlyMap<string, SearchSnapshotEntry>;
}

/** A query term, with what it stands for beyond its text. */
interface Term {
  text: string;
  /** The PR number it reads as (termPrNumber), matched exactly. */
  number: number | null;
  /** A bare `pr`, as in `pr 123` or `PR #123`: any item with a PR has it. */
  anyPr: boolean;
}

/** Per-search scratch space, so scoring a project allocates only what a
 *  result keeps. */
interface Scratch {
  /** Each term's best weight in the project being scored. */
  best: number[];
  /** The reason the last scored project shows, if any. */
  reason: MatchReason | undefined;
}

const NO_STRINGS: readonly string[] = [];
const NO_ITEMS: readonly SearchItem[] = [];
const NO_REASONS: ReadonlyMap<string, MatchReason> = new Map();

function repoPath(githubRepo: string, subpath: string | null): string {
  return subpath ? `${githubRepo}/${subpath}` : githubRepo;
}

/**
 * A label for every one of the project's repos, lower-cased: the path (and a
 * fork PR's head repo) and the badge short name. Before the project's repos
 * hydrate, the primary repo stands in, as on the grid card.
 */
function repoLabels(project: Project, ctx: SearchContext): string[] {
  const labels: string[] = [];
  const addRepo = (githubRepo: string, subpath: string | null) => {
    labels.push(repoPath(githubRepo, subpath).toLowerCase());
    const shortName = ctx.repoShortName?.(githubRepo, subpath);
    if (shortName) labels.push(shortName.toLowerCase());
  };
  const repos = ctx.reposByProject.get(project.id) ?? [];
  for (const r of repos) {
    addRepo(r.githubRepo, r.subpath);
    if (r.headRepo) labels.push(repoPath(r.headRepo, r.subpath).toLowerCase());
  }
  if (repos.length === 0 && project.githubRepo) addRepo(project.githubRepo, project.subpath);
  return labels;
}

/**
 * The project's items: one per branch (its name, PR number and PR title
 * together, so the reason line and the one-item bonus see a branch and its
 * PR as one thing), then whatever the index holds for it.
 */
function projectItems(
  branches: readonly Branch[] | undefined,
  indexItems: readonly ProjectSearchIndexItem[] | undefined
): SearchItem[] {
  const items: SearchItem[] = [];
  for (const b of branches ?? []) {
    // A title left on a branch whose PR was unlinked isn't its own.
    const prTitle = b.prNumber === null ? null : b.prTitle || null;
    items.push({
      kind: b.prNumber === null ? 'branch' : 'pr',
      name: b.branchName,
      prTitle,
      prNumber: b.prNumber,
      updatedAt: b.updatedAt,
      text: b.branchName.toLowerCase(),
      weight: FIELD_WEIGHTS.branch,
      prText: prTitle?.toLowerCase() ?? null,
    });
  }
  for (const item of indexItems ?? []) {
    items.push({
      kind: item.kind,
      name: item.title,
      prTitle: null,
      prNumber: null,
      updatedAt: item.updatedAt,
      text: item.text,
      weight: FIELD_WEIGHTS[item.kind],
      prText: null,
    });
  }
  return items;
}

function sameStrings(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((s, i) => s === b[i]);
}

/** Whether two item lists match and label alike (the lower-cased texts and
 *  weights follow from the rest). */
function sameItems(a: readonly SearchItem[], b: readonly SearchItem[]): boolean {
  return (
    a.length === b.length &&
    a.every((x, i) => {
      const y = b[i];
      return (
        x.kind === y.kind &&
        x.name === y.name &&
        x.prTitle === y.prTitle &&
        x.prNumber === y.prNumber &&
        x.updatedAt === y.updatedAt
      );
    })
  );
}

function projectEntry(
  project: Project,
  ctx: SearchContext,
  prev: SearchSnapshotEntry | undefined
): SearchSnapshotEntry {
  const name = projectDisplayName(project).toLowerCase();
  const repos = repoLabels(project, ctx);
  const branches = ctx.branchesByProject.get(project.id);
  const indexItems = ctx.index?.get(project.id);
  let items: readonly SearchItem[];
  if (prev && prev.branches === branches && prev.indexItems === indexItems) {
    items = prev.items;
  } else {
    const built = projectItems(branches, indexItems);
    items = prev && sameItems(prev.items, built) ? prev.items : built;
  }
  if (prev && items === prev.items && name === prev.name && sameStrings(prev.repos, repos)) {
    prev.branches = branches;
    prev.indexItems = indexItems;
    return prev;
  }
  return { name, repos, items, branches, indexItems };
}

/**
 * `projects` and everything `ctx` holds for them, prepared for
 * searchProjects: lower-cased once here rather than per keystroke.
 *
 * Pass the snapshot this returned last time as `previous`: when nothing a
 * search reads — names, repo labels, branch names, PR numbers and titles,
 * index titles, and the recency that picks a reason — has changed,
 * `previous` itself comes back, so a `$derived` over it doesn't invalidate
 * the results when, say, a PR status poll replaces every branch object with
 * one whose checks moved. Projects missing from the snapshot match on their
 * name alone.
 */
export function buildSearchSnapshot(
  projects: readonly Project[],
  ctx: SearchContext,
  previous?: SearchSnapshot | null
): SearchSnapshot {
  const entries = new Map<string, SearchSnapshotEntry>();
  let unchanged = !!previous && previous.entries.size === projects.length;
  for (const project of projects) {
    const prev = previous?.entries.get(project.id);
    const entry = projectEntry(project, ctx, prev);
    if (entry !== prev) unchanged = false;
    entries.set(project.id, entry);
  }
  return unchanged && previous ? previous : { entries };
}

function nameOnlyEntry(project: Project): SearchSnapshotEntry {
  return {
    name: projectDisplayName(project).toLowerCase(),
    repos: NO_STRINGS,
    items: NO_ITEMS,
    branches: undefined,
    indexItems: undefined,
  };
}

function parseTerms(terms: readonly string[]): Term[] {
  return terms.map((text) => ({ text, number: termPrNumber(text), anyPr: text === 'pr' }));
}

function includedIn(fields: readonly string[], text: string): boolean {
  for (const field of fields) if (field.includes(text)) return true;
  return false;
}

/** Whether the item holds the term through its PR prefix: `pr`, or its number. */
function itemPrHasTerm(item: SearchItem, term: Term): boolean {
  return item.prNumber !== null && (term.anyPr || term.number === item.prNumber);
}

/** What the term is worth in the item: the PR's weight when its PR holds it
 *  (itemPrHasTerm, or the PR's title), else its text's, else 0. */
function itemWeight(item: SearchItem, term: Term): number {
  if (itemPrHasTerm(item, term) || item.prText?.includes(term.text)) return FIELD_WEIGHTS.pr;
  return item.text.includes(term.text) ? item.weight : 0;
}

/** The weight of the best field in the entry that holds the term, or 0. */
function termWeight(entry: SearchSnapshotEntry, term: Term): number {
  if (entry.name.includes(term.text)) return FIELD_WEIGHTS.name;
  let best = 0;
  for (const item of entry.items) {
    best = Math.max(best, itemWeight(item, term));
    // No item field outranks a PR.
    if (best === FIELD_WEIGHTS.pr) return best;
  }
  if (best < FIELD_WEIGHTS.repo && includedIn(entry.repos, term.text)) return FIELD_WEIGHTS.repo;
  return best;
}

/** A term the name or a repo label explains: the card or row already shows it. */
function isIdentityWeight(weight: number): boolean {
  return weight === FIELD_WEIGHTS.name || weight === FIELD_WEIGHTS.repo;
}

/**
 * The reason line for `item`: its label, with the terms it matched for
 * itself (MatchReason.itemTerms). A branch with a PR is labelled by the PR —
 * `PR #123 · <title>` — but by its branch name when that alone holds the
 * item's terms, so the match shows, and until the first poll brings the
 * title.
 */
function reasonFor(
  item: SearchItem,
  terms: readonly Term[],
  best: readonly number[],
  more: number
): MatchReason {
  const itemTerms: string[] = [];
  let inTitle = false;
  let inText = false;
  for (let i = 0; i < terms.length; i++) {
    const term = terms[i];
    if (isIdentityWeight(best[i]) || itemPrHasTerm(item, term)) continue;
    const title = item.prText?.includes(term.text) ?? false;
    const text = item.text.includes(term.text);
    if (!title && !text) continue;
    itemTerms.push(term.text);
    inTitle ||= title;
    inText ||= text;
  }
  let label = item.name;
  if (item.prNumber !== null) {
    const byBranchName = item.prTitle === null || (inText && !inTitle);
    label = `PR #${item.prNumber} · ${byBranchName ? item.name : item.prTitle}`;
  }
  return { kind: item.kind, label, more, itemTerms };
}

/**
 * The entry's score for the parsed query (FIELD_WEIGHTS, MATCH_BONUS), or 0
 * when it doesn't match; its reason, if it shows one, in `scratch.reason`.
 *
 * A project matches when every term is a substring of some field of it —
 * the name, a repo label, any branch name, PR or title — case-insensitively,
 * in any order, however the terms spread across them. A number term (`123`,
 * `#123`, `pr123`, `pr#123`) also matches a branch whose PR is exactly that
 * number, and a bare `pr` any branch with a PR, so `pr 123` and `PR #123`
 * find PR 123 the way its label reads.
 *
 * Every term in the name: the top score, and no reason, as the name on
 * screen explains it. Otherwise the reason is the best of the items that
 * contributed — that hold a term at its best weight, one the name or a repo
 * label doesn't explain, or that earn the one-item bonus — ranked by what
 * the item alone would score, then recency, with the rest counted as `more`.
 * A bare `pr` met only by a PR's prefix doesn't make that PR contribute when
 * the query has other terms of its own: every PR meets it. Nothing
 * contributing means the name and repos explain the match: no reason.
 */
function matchEntry(entry: SearchSnapshotEntry, terms: readonly Term[], scratch: Scratch): number {
  const { best } = scratch;
  scratch.reason = undefined;
  let score = 0;
  let allInName = true;
  let ownTerms = 0;
  for (let i = 0; i < terms.length; i++) {
    const weight = termWeight(entry, terms[i]);
    if (weight === 0) return 0;
    best[i] = weight;
    score += weight;
    if (weight !== FIELD_WEIGHTS.name) allInName = false;
    if (!isIdentityWeight(weight)) ownTerms++;
  }
  if (allInName) return score + MATCH_BONUS.allInName;

  let shown: SearchItem | null = null;
  let shownScore = 0;
  let contributing = 0;
  let oneItem = false;
  for (const item of entry.items) {
    let held = 0;
    let itemScore = 0;
    let supplies = false;
    for (let i = 0; i < terms.length; i++) {
      const term = terms[i];
      const weight = itemWeight(item, term);
      if (weight === 0) continue;
      held++;
      itemScore += weight;
      // An item weight never equals the name's or a repo's, so this is a
      // term only items explain, at its best.
      if (weight === best[i] && !(term.anyPr && ownTerms > 1 && itemPrHasTerm(item, term))) {
        supplies = true;
      }
    }
    const whole = terms.length > 1 && held === terms.length;
    if (whole) {
      oneItem = true;
      itemScore += MATCH_BONUS.allInOneItem;
    }
    if (!supplies && !whole) continue;
    contributing++;
    if (
      !shown ||
      itemScore > shownScore ||
      (itemScore === shownScore && item.updatedAt > shown.updatedAt)
    ) {
      shown = item;
      shownScore = itemScore;
    }
  }
  if (oneItem) score += MATCH_BONUS.allInOneItem;
  if (shown) scratch.reason = reasonFor(shown, terms, best, contributing - 1);
  return score;
}

/** `kept` best score first; Array#sort is stable, so ties keep input order. */
function byScore(kept: Project[], scores: readonly number[]): Project[] {
  return kept
    .map((_, i) => i)
    .sort((a, b) => scores[b] - scores[a])
    .map((i) => kept[i]);
}

/**
 * The projects the query keeps, by matchEntry's rule, ranked, and why, in
 * one pass. Without a snapshot only names are searched. An empty (or
 * whitespace-only) query keeps every project — the input array, unchanged,
 * with no rank — and, like a query that no item explained, gets the same
 * empty reasons map every time, so a `$derived` over it stays put.
 */
export function searchProjects(
  projects: Project[],
  query: string,
  snapshot?: SearchSnapshot | null
): ProjectSearchResult {
  const raw = queryTerms(query);
  if (raw.length === 0) return { projects, rank: null, reasons: NO_REASONS };
  const terms = parseTerms(raw);
  const scratch: Scratch = { best: new Array<number>(terms.length).fill(0), reason: undefined };
  const kept: Project[] = [];
  const scores: number[] = [];
  let tied = true;
  let reasons: Map<string, MatchReason> | null = null;
  for (const project of projects) {
    const entry = snapshot?.entries.get(project.id) ?? nameOnlyEntry(project);
    const score = matchEntry(entry, terms, scratch);
    if (score === 0) continue;
    if (scores.length > 0 && score !== scores[0]) tied = false;
    kept.push(project);
    scores.push(score);
    if (scratch.reason) (reasons ??= new Map()).set(project.id, scratch.reason);
  }
  const ranked = tied ? kept : byScore(kept, scores);
  return {
    projects: ranked,
    rank: new Map(ranked.map((p, i) => [p.id, i])),
    reasons: reasons ?? NO_REASONS,
  };
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
 * The projects the landing-page grid shows, in the order it shows them.
 *
 * The query is applied to whatever the chips (and the sticky exception) left.
 * The sticky card is not exempt from the search: the exception exists so a
 * card the user just visited doesn't vanish because visiting it changed its
 * read state, whereas a typed query is a deliberate narrowing, so a sticky
 * card that doesn't match it is hidden. Nor is it pinned first: it ranks
 * like any other card. The query outlives the visit, so the card comes back
 * to the rank it was opened from, which is where the scroll restore looks
 * for it.
 *
 * With no sticky re-insertion and an empty query, `chipFiltered` is returned
 * unchanged as the result's `projects`.
 */
export function visibleProjects(
  projects: Project[],
  chipFiltered: Project[],
  stickyId: string | null,
  query: string,
  snapshot?: SearchSnapshot | null
): ProjectSearchResult {
  return searchProjects(withStickyProject(projects, chipFiltered, stickyId), query, snapshot);
}

/**
 * The ⌘1–9 number of each card on screen, by project id: the first nine of
 * `shown` (visibleProjects' result), in the order the grid lays them out —
 * rank order while searching. The grid keeps cards mounted in their
 * unsearched order, hiding and reordering them with CSS, so a card's place in
 * the rendered list is not its place on screen; ⌘N opens `shown[N - 1]`, and
 * these are the numbers the overlay has to show for that.
 */
export function projectShortcutNumbers(shown: Project[]): Map<string, number> {
  return new Map(shown.slice(0, 9).map((p, i) => [p.id, i + 1]));
}
