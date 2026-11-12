/**
 * Client-side text search for the landing-page project grid and the sidebar.
 *
 * Plain module (no runes, no store imports) so the matching and ranking rules
 * stay trivially unit-testable; the caller hands in what to search beyond the
 * project names as a SearchContext. Applied after the chip filters in
 * ProjectsList.
 *
 * The work is split by how often it runs. `buildSearchSnapshot` runs when
 * the data changes: it lower-cases every name, repo label, branch, tag and
 * title once, and hands back the previous snapshot when nothing matching
 * reads has moved, so a PR status poll (fresh branch objects, same names and
 * titles) doesn't re-run a search held on screen. `searchProjects` runs per
 * keystroke: it parses the query once, tests the prepared strings in place,
 * scores each match (FIELD_WEIGHTS), builds a label only for the item each
 * result shows, and returns the ranked matches and their reasons from one
 * pass.
 *
 * Neither surface re-renders per keystroke. Both keep every row mounted,
 * hide what the query rules out, and lay the rest out by
 * `ProjectSearchResult.rank` through CSS `order`, so a keystroke flips
 * attributes and styles on rows that already exist rather than re-keying an
 * `{#each}`; once the query settles, the rows are re-keyed into rank order
 * once, so DOM order matches the screen (Row order, searchRowOrder). A
 * held query is re-ranked only when it or the chip filters are edited
 * (searchOrderHold).
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
// name, or MATCH_BONUS.allInOneItem when a single branch (with its PR), tag,
// note or review holds every term of a multi-word query. The weights are
// powers of two, so a one-word query orders strictly by the best field it
// hit; tags and repo labels share a tier.
//
// A bare `pr` is a PR term. In a project with a PR it scores as the PR,
// however many words of the name contain the two letters (improved, project,
// prod); in one without, it is plain text, and the name still matches it
// but scores nothing for it, so every project with a PR outranks it.
//
// Ranking is by field; the reason line is by identity. A term the name or a
// repo label holds is one the card already shows, so it never puts an item
// on the reason line, even where an item's field outscores the repo label
// and lifts the card.
//
// A project's tags are items, not identity: they score with the repo labels,
// as both are labels on the card, but a tag that explains a match gets the
// reason line. Sidebar rows show no tag chips, and a grid card shows its
// icon group's tag only as an icon and at most three chips.

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
  /** The name of one of the project's tags. */
  tag: 8,
  /** A project note's title. */
  projectNote: 4,
  /** A branch note's title. */
  note: 2,
  /** A review's title. */
  review: 1,
} as const;

/** A field a query term can be found in. */
export type SearchField = keyof typeof FIELD_WEIGHTS;

/** The heaviest field an item can hold a term in: a term's scan of the
 *  items stops at the first item holding it there. */
const MAX_ITEM_WEIGHT = Math.max(
  FIELD_WEIGHTS.pr,
  FIELD_WEIGHTS.branch,
  FIELD_WEIGHTS.tag,
  FIELD_WEIGHTS.projectNote,
  FIELD_WEIGHTS.note,
  FIELD_WEIGHTS.review
);

/** One bit per field, so the fields an item holds text in and the fields a
 *  query's terms were best found in compare in one AND. */
const FIELD_BIT: Record<SearchField, number> = {
  name: 1,
  pr: 2,
  branch: 4,
  repo: 8,
  projectNote: 16,
  note: 32,
  review: 64,
  tag: 128,
};

/** Added once to a project's score when every term lands in one place. */
export const MATCH_BONUS = {
  /** Every term is in the project's name. */
  allInName: 64,
  /** One branch, tag, note or review holds every term. Multi-word queries only:
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
  /** The names of the project's tags, pending writes included (tagsStore). */
  tagNames?: (projectId: string) => readonly string[];
}

export type MatchReasonKind = 'branch' | 'pr' | 'tag' | ProjectSearchItemKind;

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
   * The query terms (parsed: repeats and contained words dropped) this
   * item matched for itself: the ones it holds that neither the name nor a
   * repo label holds, minus any its PR prefix explains (a bare `pr`, or the
   * PR's number). The label
   * windows on the first of these, so a term that highlights early anyway —
   * in the `PR #123` prefix, or one the name already matched — doesn't hold
   * a long title's match off screen.
   */
  itemTerms: readonly string[];
}

/** A search's result: what it keeps, in what order, and why, from one pass. */
export interface ProjectSearchResult {
  /** The projects the query keeps, best match first and equal scores in
   *  input order (or, after searchOrderHold, the order already on screen);
   *  for a blank query, the input array itself. */
  projects: Project[];
  /** Each kept project's index in `projects`, by id: the order the surfaces
   *  lay kept rows out in, and the set they hide the rest by. Null for a
   *  blank query, which keeps everything in its own order. */
  rank: ReadonlyMap<string, number> | null;
  /** Why each kept project matched, by id, when its name and repos alone
   *  don't say. */
  reasons: ReadonlyMap<string, MatchReason>;
}

/** A searchable thing inside a project, its text lower-cased in advance. */
interface SearchItem {
  kind: MatchReasonKind;
  /** The branch name, tag name or title, as shown. */
  name: string;
  /** The PR's title, on a branch with a PR once a poll or link brings it. */
  prTitle: string | null;
  /** Matched exactly by a number term, never as text: `12` must not find PR 123. */
  prNumber: number | null;
  updatedAt: number;
  /** `name` lower-cased: a branch name, tag name or title. */
  text: string;
  /** The field `text` is: a branch name, a tag, or the index item's kind. */
  field: SearchField;
  /** `prTitle` lower-cased, in the `pr` field. */
  prText: string | null;
  /** FIELD_BIT of every field the item can hold a term in: `field`, and
   *  `pr` when it has a PR. */
  fields: number;
}

/** One project's prepared search data. */
export interface SearchSnapshotEntry {
  /** The project's name, lower-cased. */
  readonly name: string;
  /** Its repo labels, lower-cased. */
  readonly repos: readonly string[];
  readonly items: readonly SearchItem[];
  /** Whether any item has a PR, for a bare `pr`. */
  readonly hasPr: boolean;
  /** The context lists `items` came from. Bookkeeping, updated in place when
   *  an entry is kept, so an unchanged list skips the item-by-item compare. */
  branches: readonly Branch[] | undefined;
  indexItems: readonly ProjectSearchIndexItem[] | undefined;
  /** The project's tag names, the source of its tag items. */
  tagNames: readonly string[];
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
 *  result keeps. Indexed by term, for the project being scored. */
interface Scratch {
  /** The best field the term was found in. */
  field: SearchField[];
  /** What the term scores: its best field's weight, or 0 for a bare `pr`
   *  that only the name of a project without a PR holds. */
  weight: number[];
  /** Whether the name or a repo label holds the term: the card already
   *  shows it, so no item has to explain it. */
  identity: boolean[];
  /** The reason the last scored project shows, if any. */
  reason: MatchReason | undefined;
}

/** matchEntry's score for an entry that doesn't match. A match can score 0:
 *  a bare `pr` only a PR-less project's name holds. */
const NO_MATCH = -1;

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
 * PR as one thing), then whatever the index holds for it, then one per tag.
 */
function projectItems(
  branches: readonly Branch[] | undefined,
  indexItems: readonly ProjectSearchIndexItem[] | undefined,
  tagNames: readonly string[]
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
      field: 'branch',
      prText: prTitle?.toLowerCase() ?? null,
      fields: FIELD_BIT.branch | (b.prNumber === null ? 0 : FIELD_BIT.pr),
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
      field: item.kind,
      prText: null,
      fields: FIELD_BIT[item.kind],
    });
  }
  for (const name of tagNames) {
    // Tags carry no recency: on a score tie a dated item shows over a tag,
    // and of two tags, the first in catalog order.
    items.push({
      kind: 'tag',
      name,
      prTitle: null,
      prNumber: null,
      updatedAt: 0,
      text: name.toLowerCase(),
      field: 'tag',
      prText: null,
      fields: FIELD_BIT.tag,
    });
  }
  return items;
}

function sameStrings(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((s, i) => s === b[i]);
}

/** Whether two item lists match and label alike (the lower-cased texts and
 *  fields follow from the rest). */
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
  const tagNames = ctx.tagNames?.(project.id) ?? NO_STRINGS;
  let items: readonly SearchItem[];
  if (
    prev &&
    prev.branches === branches &&
    prev.indexItems === indexItems &&
    sameStrings(prev.tagNames, tagNames)
  ) {
    items = prev.items;
  } else {
    const built = projectItems(branches, indexItems, tagNames);
    items = prev && sameItems(prev.items, built) ? prev.items : built;
  }
  if (prev && items === prev.items && name === prev.name && sameStrings(prev.repos, repos)) {
    prev.branches = branches;
    prev.indexItems = indexItems;
    prev.tagNames = tagNames;
    return prev;
  }
  const hasPr = items.some((item) => item.prNumber !== null);
  return { name, repos, items, hasPr, branches, indexItems, tagNames };
}

/**
 * `projects` and everything `ctx` holds for them, prepared for
 * searchProjects: lower-cased once here rather than per keystroke.
 *
 * Pass the snapshot this returned last time as `previous`: when nothing a
 * search reads — names, repo labels, branch names, PR numbers and titles,
 * index titles, tag names, and the recency that picks a reason — has changed,
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
    hasPr: false,
    branches: undefined,
    indexItems: undefined,
    tagNames: NO_STRINGS,
  };
}

/**
 * Whether every project `other` matches, `term` matches too, so `term` adds
 * nothing. Only text a term is matched by as text can imply anything:
 *
 * - A number term can match a PR by its number rather than its text, so it
 *   is never implied, only repeated.
 * - A bare `pr` matches any project with a PR whatever its text holds, so
 *   it implies nothing (`p pr` keeps `p`). It is implied only by a number
 *   term it starts (`pr123`, `pr#12`): whatever matches that either has
 *   the PR or holds the letters.
 * - Plain text is implied by plain text holding it (`log` by `login`),
 *   never by a number term, which might have matched by number, nor by a
 *   bare `pr`, which might have matched by a PR.
 */
function impliedBy(term: Term, other: Term): boolean {
  if (term.number !== null || !other.text.includes(term.text)) return false;
  if (term.anyPr) return other.number !== null;
  return other.number === null && !other.anyPr;
}

/**
 * The query's terms, parsed, minus any another term implies (impliedBy): a
 * repeat, or a word inside a longer one (`log` in `login`, `pr` in
 * `pr123`). What matches is unchanged; what stops is one word counting
 * twice, scoring twice and earning the one-item bonus a one-word query
 * never gets, so `login login` ranks as `login` does. Of two repeats, the
 * first is kept.
 */
function parseTerms(raw: readonly string[]): Term[] {
  const all = raw.map((text) => ({ text, number: termPrNumber(text), anyPr: text === 'pr' }));
  if (all.length === 1) return all;
  return all.filter(
    (term, i) =>
      !all.some((other, j) =>
        other.text === term.text ? j < i : j !== i && impliedBy(term, other)
      )
  );
}

function includedIn(fields: readonly string[], text: string): boolean {
  for (const field of fields) if (field.includes(text)) return true;
  return false;
}

/** Whether the item holds the term through its PR prefix: `pr`, or its number. */
function itemPrHasTerm(item: SearchItem, term: Term): boolean {
  return item.prNumber !== null && (term.anyPr || term.number === item.prNumber);
}

/** The field the item holds the term in: its PR when the PR does
 *  (itemPrHasTerm, or the PR's title), else its text's, else null. */
function itemField(item: SearchItem, term: Term): SearchField | null {
  if (itemPrHasTerm(item, term) || item.prText?.includes(term.text)) return 'pr';
  return item.text.includes(term.text) ? item.field : null;
}

/**
 * Records term `i`'s best field in the entry, what it scores, and whether
 * the name or a repo label holds it (Scratch). False when nothing holds it.
 */
function scoreTerm(entry: SearchSnapshotEntry, term: Term, i: number, s: Scratch): boolean {
  if (term.anyPr && entry.hasPr) {
    // The project's PRs hold it, whatever else has the two letters.
    s.field[i] = 'pr';
    s.weight[i] = FIELD_WEIGHTS.pr;
    s.identity[i] = false;
    return true;
  }
  const inName = entry.name.includes(term.text);
  const inRepo = includedIn(entry.repos, term.text);
  let field: SearchField | null = inName ? 'name' : null;
  // A bare `pr` here is two letters of a word; see Ranking.
  let weight = inName && !term.anyPr ? FIELD_WEIGHTS.name : 0;
  if (weight < MAX_ITEM_WEIGHT) {
    for (const item of entry.items) {
      const found = itemField(item, term);
      if (found === null || FIELD_WEIGHTS[found] <= weight) continue;
      field = found;
      weight = FIELD_WEIGHTS[found];
      if (weight === MAX_ITEM_WEIGHT) break;
    }
  }
  if (inRepo && FIELD_WEIGHTS.repo > weight) {
    field = 'repo';
    weight = FIELD_WEIGHTS.repo;
  }
  if (field === null) return false;
  s.field[i] = field;
  s.weight[i] = weight;
  s.identity[i] = inName || inRepo;
  return true;
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
  identity: readonly boolean[],
  more: number
): MatchReason {
  const itemTerms: string[] = [];
  let inTitle = false;
  let inText = false;
  for (let i = 0; i < terms.length; i++) {
    const term = terms[i];
    if (identity[i] || itemPrHasTerm(item, term)) continue;
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
 * The entry's score for the parsed query (FIELD_WEIGHTS, MATCH_BONUS), or
 * NO_MATCH; its reason, if it shows one, in `s.reason`.
 *
 * A project matches when every term is a substring of some field of it —
 * the name, a repo label, any branch name, PR, tag or title —
 * case-insensitively, in any order, however the terms spread across them. A number term (`123`,
 * `#123`, `pr123`, `pr#123`) also matches a branch whose PR is exactly that
 * number, and a bare `pr` any branch with a PR, so `pr 123` and `PR #123`
 * find PR 123 the way its label reads.
 *
 * Every term in the name: the top score, and no reason, as the name on
 * screen explains it. Otherwise the reason is the best of the items that
 * contributed, ranked by what the item alone would score, then recency,
 * with the rest counted as `more`. An item contributes when it holds one of
 * the query's own terms — one neither the name nor a repo label holds — in
 * that term's best field, or holds every term of a query that has one of
 * its own. A bare `pr` met only by a PR's prefix doesn't make that PR
 * contribute when the query has other terms of its own: every PR meets it.
 * A query with no terms of its own is the name and repos' match: no reason,
 * though items can still lift its score.
 */
function matchEntry(entry: SearchSnapshotEntry, terms: readonly Term[], s: Scratch): number {
  s.reason = undefined;
  let score = 0;
  let allInName = true;
  let ownTerms = 0;
  // FIELD_BIT of the best fields of the query's own terms: what an item
  // has to hold text in to supply one.
  let wanted = 0;
  for (let i = 0; i < terms.length; i++) {
    if (!scoreTerm(entry, terms[i], i, s)) return NO_MATCH;
    score += s.weight[i];
    if (s.field[i] !== 'name' || terms[i].anyPr) allInName = false;
    if (!s.identity[i]) {
      ownTerms++;
      wanted |= FIELD_BIT[s.field[i]];
    }
  }
  if (allInName) return score + MATCH_BONUS.allInName;
  const multi = terms.length > 1;
  // One term, and the name or a repo holds it: no item explains anything.
  if (!multi && ownTerms === 0) return score;

  let shown: SearchItem | null = null;
  let shownScore = 0;
  let contributing = 0;
  let oneItem = false;
  for (const item of entry.items) {
    const canSupply = (item.fields & wanted) !== 0;
    // Otherwise it only counts by holding every term: as a contributor when
    // the query has terms of its own, else just for the bonus, once.
    if (!canSupply && !(multi && (ownTerms > 0 || !oneItem))) continue;
    let held = 0;
    let itemScore = 0;
    let supplies = false;
    for (let i = 0; i < terms.length; i++) {
      const term = terms[i];
      const found = itemField(item, term);
      if (found === null) {
        // Missing one term, it can't hold every term either.
        if (!canSupply) break;
        continue;
      }
      held++;
      itemScore += FIELD_WEIGHTS[found];
      if (
        !s.identity[i] &&
        found === s.field[i] &&
        !(term.anyPr && ownTerms > 1 && itemPrHasTerm(item, term))
      ) {
        supplies = true;
      }
    }
    const whole = multi && held === terms.length;
    if (whole) {
      oneItem = true;
      itemScore += MATCH_BONUS.allInOneItem;
    }
    if (!supplies && !(whole && ownTerms > 0)) continue;
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
  if (shown) s.reason = reasonFor(shown, terms, s.identity, contributing - 1);
  return score;
}

/** `kept` best score first; Array#sort is stable, so ties keep input order. */
function byScore(kept: Project[], scores: readonly number[]): Project[] {
  return kept
    .map((_, i) => i)
    .sort((a, b) => scores[b] - scores[a])
    .map((i) => kept[i]);
}

/** Each project's index in `projects`, by id. */
function positions(projects: readonly Project[]): Map<string, number> {
  const rank = new Map<string, number>();
  for (let i = 0; i < projects.length; i++) rank.set(projects[i].id, i);
  return rank;
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
  const scratch: Scratch = {
    field: new Array<SearchField>(terms.length).fill('name'),
    weight: new Array<number>(terms.length).fill(0),
    identity: new Array<boolean>(terms.length).fill(false),
    reason: undefined,
  };
  const kept: Project[] = [];
  const scores: number[] = [];
  let tied = true;
  let reasons: Map<string, MatchReason> | null = null;
  for (const project of projects) {
    const entry = snapshot?.entries.get(project.id) ?? nameOnlyEntry(project);
    const score = matchEntry(entry, terms, scratch);
    if (score === NO_MATCH) continue;
    if (scores.length > 0 && score !== scores[0]) tied = false;
    kept.push(project);
    scores.push(score);
    if (scratch.reason) (reasons ??= new Map()).set(project.id, scratch.reason);
  }
  const ranked = tied ? kept : byScore(kept, scores);
  return { projects: ranked, rank: positions(ranked), reasons: reasons ?? NO_REASONS };
}

function sameIds(a: readonly Project[], b: readonly Project[]): boolean {
  return a.length === b.length && a.every((p, i) => p.id === b[i].id);
}

/** What searchOrderHold remembers about the query it holds. */
interface HeldOrder {
  query: string;
  /** The chip filters the query was ranked under, by identity. */
  filters: ReadonlySet<string>;
  /** Every id ranked under `query`, in the order on screen, including ones
   *  that have since stopped matching: one that matches again goes back to
   *  its slot. Ids first seen later are appended. */
  order: string[];
  seen: Set<string>;
  /** The result last handed out. */
  last: ProjectSearchResult;
}

/**
 * `next`'s matches in `held`'s remembered order, then the ones never seen
 * under the query in `next`'s order, which `held` then remembers too.
 * `held.last` again when nothing changed, its rank map with `next`'s reasons
 * when only the reasons or project objects did, and `next` itself when its
 * own order is the held one. `next.rank` is non-null.
 */
function inHeldOrder(held: HeldOrder, next: ProjectSearchResult): ProjectSearchResult {
  const nextRank = next.rank!;
  const projects: Project[] = [];
  for (const id of held.order) {
    const at = nextRank.get(id);
    if (at !== undefined) projects.push(next.projects[at]);
  }
  for (const p of next.projects) {
    if (held.seen.has(p.id)) continue;
    held.seen.add(p.id);
    held.order.push(p.id);
    projects.push(p);
  }
  const last = held.last;
  // The same rows in the same places: keep the rank map, so what reads it —
  // `hidden` on every row, the settle wait (searchRowOrder) — stays put
  // through a PR poll or an index refetch that only changed a reason.
  if (sameIds(projects, last.projects)) {
    const same = projects.every((p, i) => p === last.projects[i]);
    if (same && next.reasons === last.reasons) return last;
    return { projects: same ? last.projects : projects, rank: last.rank, reasons: next.reasons };
  }
  if (projects.every((p, i) => p === next.projects[i])) return next;
  return { projects, rank: positions(projects), reasons: next.reasons };
}

/**
 * A surface's search results, re-ranked only when the user edits the search:
 * its query, or its chip filters. Call the returned function with every fresh
 * result, the query it ran for and the active filter Set
 * (projectFiltersStore.activeFilters), inside the `$derived` that runs the
 * search.
 *
 * While the query and the filter Set stay the ones the results were last
 * ranked for, a fresh result keeps the order already on screen: projects
 * that still match keep their places, ones that stopped matching drop out,
 * and new matches follow the rest, best first. A project that drops out and
 * matches again — the sticky return card, after visiting it marked it read
 * and the Unread chip dropped it — goes back to the slot it had, not to the
 * end. So a PR poll bringing a title, an index refetch while a session
 * writes notes, a rename or a read-state change under a held query can hide
 * a card or add one at the end, but never move the card under the cursor,
 * or the one ⌘N was about to open. Reasons are always the fresh result's.
 * When the rows on screen are unchanged, so is the rank map.
 *
 * Any edit to the query, a space included, ranks afresh and forgets the
 * slots, and so does a chip toggle: the store replaces the filter Set on
 * every toggle or clear and only then, so the Set's identity is the signal
 * (the grid's sticky card goes by it too). Widening the filters then ranks
 * the cards they bring back among the rest rather than after them. A blank
 * query passes through.
 */
export function searchOrderHold(): (
  query: string,
  filters: ReadonlySet<string>,
  result: ProjectSearchResult
) => ProjectSearchResult {
  let held: HeldOrder | null = null;
  return (query, filters, result) => {
    if (result.rank === null) {
      held = null;
      return result;
    }
    if (held !== null && held.query === query && held.filters === filters) {
      held.last = inHeldOrder(held, result);
      return held.last;
    }
    const order = result.projects.map((p) => p.id);
    held = { query, filters, order, seen: new Set(order), last: result };
    return result;
  };
}

// ── Row order ──
//
// A surface lays out its rows by `rank` with CSS `order` per keystroke,
// leaving the DOM alone, and re-keys its {#each} into rank order once the
// query settles, so DOM and tab order catch up with the order on screen
// (searchRowOrder.svelte.ts). These are the pure halves of that.

/**
 * `base` with the projects `order` ranks first, by position, then the rest
 * in `base` order: the order to render rows in once a search settles. A
 * null order is `base` itself.
 */
export function inRankOrder(base: Project[], order: ReadonlyMap<string, number> | null): Project[] {
  if (order === null) return base;
  const ranked: Array<Project | undefined> = new Array(order.size);
  const rest: Project[] = [];
  for (const p of base) {
    const at = order.get(p.id);
    if (at === undefined) rest.push(p);
    else ranked[at] = p;
  }
  // Positions of projects `base` no longer holds stay empty.
  const rows: Project[] = [];
  for (const p of ranked) if (p !== undefined) rows.push(p);
  for (const p of rest) rows.push(p);
  return rows;
}

/** Each project's position in `base`, by id: where a row goes for a blank
 *  query while the DOM still holds the last search's order. */
export function basePositions(base: readonly Project[]): ReadonlyMap<string, number> {
  return positions(base);
}

/**
 * Whether `rows`, in render order, already put the projects `target` ranks
 * in its order, so CSS `order` has nothing to move. Rows `target` doesn't
 * hold are hidden and don't count.
 */
export function rowsFollowRank(
  rows: readonly Project[],
  target: ReadonlyMap<string, number>
): boolean {
  let last = -1;
  for (const p of rows) {
    const at = target.get(p.id);
    if (at === undefined) continue;
    if (at < last) return false;
    last = at;
  }
  return true;
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
 * for it — even when the chips dropped it while the grid was covered, since
 * searchOrderHold remembers its slot.
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
 * rank order while searching. The grid keeps cards mounted, hiding them and,
 * until a query settles, reordering them with CSS, so a card's place in the
 * rendered list is not its place on screen; ⌘N opens `shown[N - 1]`, and
 * these are the numbers the overlay has to show for that.
 */
export function projectShortcutNumbers(shown: Project[]): Map<string, number> {
  return new Map(shown.slice(0, 9).map((p, i) => [p.id, i + 1]));
}
