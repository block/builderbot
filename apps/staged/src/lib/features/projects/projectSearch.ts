/**
 * Client-side text search for the landing-page project grid and the sidebar.
 *
 * Plain module (no runes, no store imports) so the matching rule stays
 * trivially unit-testable; the caller hands in what to search beyond the
 * project names as a SearchContext. Applied after the chip filters in
 * ProjectsList.
 *
 * The work is split by how often it runs. `buildSearchSnapshot` runs when
 * the data changes: it lower-cases every name, repo label, branch and title
 * once, and hands back the previous snapshot when nothing matching reads has
 * moved, so a PR status poll (fresh branch objects, same names and titles)
 * doesn't re-run a search held on screen. `searchProjects` runs per
 * keystroke: it parses the query once, tests the prepared strings in place,
 * builds a label only for the item each result shows, and returns the
 * matches and their reasons from one pass.
 *
 * `visibleProjects` is the whole composition the grid shows — chip filters,
 * sticky return-target card, then the query — lifted out of the component so
 * the wiring between the three is testable without a Svelte runtime. The grid
 * renders `withStickyProject` and hides the cards the query rules out, so
 * `visibleProjects` is the cards left on screen, not the ones mounted.
 */

import type { Branch, Project, ProjectRepo, ProjectSearchItemKind } from '../../types';
import { queryTerms, termPrNumber } from '../../shared/textSearch';
import { projectDisplayName } from '../../shared/utils';

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
 * best matching item and how many others matched besides.
 */
export interface MatchReason {
  kind: MatchReasonKind;
  label: string;
  /** Matching items other than the one shown. */
  more: number;
  /**
   * The query terms (queryTerms) this item had to match itself: the ones the
   * identity doesn't hold, minus any its PR prefix explains (a bare `pr`, or
   * the PR's number). The label windows on the first of these, so a term
   * that highlights early anyway — in the `PR #123` prefix, or one the name
   * already matched — doesn't hold a long title's match off screen.
   */
  itemTerms: readonly string[];
}

/** A search's result: what it keeps, and why, from one pass. */
export interface ProjectSearchResult {
  /** The projects the query keeps, in input order; for a blank query, the
   *  input array itself. */
  projects: Project[];
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
  /** `name` and `prTitle` lower-cased: what the terms are tested against. */
  texts: readonly string[];
}

/** One project's prepared search data. */
export interface SearchSnapshotEntry {
  /** Name and repo labels, lower-cased. */
  readonly identity: readonly string[];
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

// Which item explains a match best, lowest first: branches and PRs, then
// branch notes, project notes and reviews. Ties go to the most recent.
const KIND_PRIORITY: Record<MatchReasonKind, number> = {
  branch: 0,
  pr: 0,
  note: 1,
  projectNote: 2,
  review: 3,
};

const NO_ITEMS: readonly SearchItem[] = [];
const NO_REASONS: ReadonlyMap<string, MatchReason> = new Map();

function repoPath(githubRepo: string, subpath: string | null): string {
  return subpath ? `${githubRepo}/${subpath}` : githubRepo;
}

/**
 * The project's identity, lower-cased: its name plus a label for every repo —
 * the path (and a fork PR's head repo) and the badge short name. Before the
 * project's repos hydrate, the primary repo stands in, as on the grid card.
 */
function identityFields(project: Project, ctx: SearchContext): string[] {
  const fields = [projectDisplayName(project).toLowerCase()];
  const addRepo = (githubRepo: string, subpath: string | null) => {
    fields.push(repoPath(githubRepo, subpath).toLowerCase());
    const shortName = ctx.repoShortName?.(githubRepo, subpath);
    if (shortName) fields.push(shortName.toLowerCase());
  };
  const repos = ctx.reposByProject.get(project.id) ?? [];
  for (const r of repos) {
    addRepo(r.githubRepo, r.subpath);
    if (r.headRepo) fields.push(repoPath(r.headRepo, r.subpath).toLowerCase());
  }
  if (repos.length === 0 && project.githubRepo) addRepo(project.githubRepo, project.subpath);
  return fields;
}

/**
 * The project's items: one per branch (its name, PR number and PR title
 * together, so a query can't pair one branch's name with another's PR), then
 * whatever the index holds for it.
 */
function projectItems(
  branches: readonly Branch[] | undefined,
  indexItems: readonly ProjectSearchIndexItem[] | undefined
): SearchItem[] {
  const items: SearchItem[] = [];
  for (const b of branches ?? []) {
    // A title left on a branch whose PR was unlinked isn't its own.
    const prTitle = b.prNumber === null ? null : b.prTitle || null;
    const name = b.branchName.toLowerCase();
    items.push({
      kind: b.prNumber === null ? 'branch' : 'pr',
      name: b.branchName,
      prTitle,
      prNumber: b.prNumber,
      updatedAt: b.updatedAt,
      texts: prTitle ? [name, prTitle.toLowerCase()] : [name],
    });
  }
  for (const item of indexItems ?? []) {
    items.push({
      kind: item.kind,
      name: item.title,
      prTitle: null,
      prNumber: null,
      updatedAt: item.updatedAt,
      texts: [item.text],
    });
  }
  return items;
}

function sameStrings(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((s, i) => s === b[i]);
}

/** Whether two item lists match and label alike (`texts` follow from the rest). */
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
  const identity = identityFields(project, ctx);
  const branches = ctx.branchesByProject.get(project.id);
  const indexItems = ctx.index?.get(project.id);
  let items: readonly SearchItem[];
  if (prev && prev.branches === branches && prev.indexItems === indexItems) {
    items = prev.items;
  } else {
    const built = projectItems(branches, indexItems);
    items = prev && sameItems(prev.items, built) ? prev.items : built;
  }
  if (prev && items === prev.items && sameStrings(prev.identity, identity)) {
    prev.branches = branches;
    prev.indexItems = indexItems;
    return prev;
  }
  return { identity, items, branches, indexItems };
}

/**
 * `projects` and everything `ctx` holds for them, prepared for
 * searchProjects: lower-cased once here rather than per keystroke.
 *
 * Pass the snapshot this returned last time as `previous`: when nothing a
 * search reads — names, repo labels, branch names, PR numbers and titles,
 * index titles, and the recency that ranks them — has changed, `previous`
 * itself comes back, so a `$derived` over it doesn't invalidate the results
 * when, say, a PR status poll replaces every branch object with one whose
 * checks moved. Projects missing from the snapshot match on their name alone.
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
    identity: [projectDisplayName(project).toLowerCase()],
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

/** Whether the item holds the term through its PR: `pr`, or its number. */
function itemPrHasTerm(item: SearchItem, term: Term): boolean {
  return item.prNumber !== null && (term.anyPr || term.number === item.prNumber);
}

function itemHasTerm(item: SearchItem, term: Term): boolean {
  return itemPrHasTerm(item, term) || includedIn(item.texts, term.text);
}

function itemHasEvery(item: SearchItem, terms: readonly Term[]): boolean {
  for (const term of terms) if (!itemHasTerm(item, term)) return false;
  return true;
}

function outranks(a: SearchItem, b: SearchItem): boolean {
  const byKind = KIND_PRIORITY[a.kind] - KIND_PRIORITY[b.kind];
  return byKind !== 0 ? byKind < 0 : a.updatedAt > b.updatedAt;
}

/**
 * A branch with a PR is labelled by the PR — `PR #123 · <title>`, or the
 * branch name until the first poll brings the title.
 */
function itemLabel(item: SearchItem): string {
  if (item.prNumber === null) return item.name;
  return `PR #${item.prNumber} · ${item.prTitle ?? item.name}`;
}

/**
 * Whether the entry matches the parsed query: `null` for no, `undefined` for
 * yes on its identity alone, else the reason.
 *
 * Every term must land somewhere, case-insensitively and in any order, but
 * not just anywhere: a project matches when its identity — name and repo
 * labels — holds every term, or when its identity together with one single
 * item does. So "staged login" finds a project in the staged repo with a
 * branch named fix-login, while "fix login" doesn't match a project just
 * because one branch says "fix" and another says "login". A number term
 * (`123`, `#123`, `pr123`, `pr#123`) also matches a branch whose PR is
 * exactly that number, and a bare `pr` any branch with a PR, so `pr 123` and
 * `PR #123` find PR 123 the way its label reads.
 *
 * Matching on identity alone gives no reason: the name or repo badge on
 * screen already explains it. Otherwise the reason is the best matching item
 * by kind (KIND_PRIORITY), then recency, counting the rest as `more`, with
 * the terms it matched beyond the identity and its PR as `itemTerms`.
 */
function matchEntry(
  entry: SearchSnapshotEntry,
  terms: readonly Term[]
): MatchReason | null | undefined {
  let missing: Term[] | null = null;
  for (const term of terms) {
    if (!includedIn(entry.identity, term.text)) (missing ??= []).push(term);
  }
  if (!missing) return undefined;

  let best: SearchItem | null = null;
  let count = 0;
  for (const item of entry.items) {
    if (!itemHasEvery(item, missing)) continue;
    count++;
    if (!best || outranks(item, best)) best = item;
  }
  if (!best) return null;
  const shown = best;
  return {
    kind: shown.kind,
    label: itemLabel(shown),
    more: count - 1,
    itemTerms: missing.filter((term) => !itemPrHasTerm(shown, term)).map((term) => term.text),
  };
}

/**
 * The projects the query keeps, by matchEntry's rule, and why, in one pass.
 * Without a snapshot only names are searched. An empty (or whitespace-only)
 * query keeps every project — the input array, unchanged — and, like a query
 * that matched only on identities, gets the same empty reasons map every
 * time, so a `$derived` over it stays put.
 */
export function searchProjects(
  projects: Project[],
  query: string,
  snapshot?: SearchSnapshot | null
): ProjectSearchResult {
  const raw = queryTerms(query);
  if (raw.length === 0) return { projects, reasons: NO_REASONS };
  const terms = parseTerms(raw);
  const kept: Project[] = [];
  let reasons: Map<string, MatchReason> | null = null;
  for (const project of projects) {
    const entry = snapshot?.entries.get(project.id) ?? nameOnlyEntry(project);
    const reason = matchEntry(entry, terms);
    if (reason === null) continue;
    kept.push(project);
    if (reason) (reasons ??= new Map()).set(project.id, reason);
  }
  return { projects: kept, reasons: reasons ?? NO_REASONS };
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
 * `shown` (visibleProjects' result) in grid order. The grid keeps cards the
 * query hides mounted, so a card's place in the rendered list is not its
 * place on screen; ⌘N opens `shown[N - 1]`, and these are the numbers the
 * overlay has to show for that.
 */
export function projectShortcutNumbers(shown: Project[]): Map<string, number> {
  return new Map(shown.slice(0, 9).map((p, i) => [p.id, i + 1]));
}
