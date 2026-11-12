/**
 * Matching, ranking and highlighting for the ⌘K project palette
 * (CommandPalette.svelte).
 *
 * Plain module — no runes, no store imports — so the rules stay unit-testable
 * without a Svelte runtime. The palette hands in every project the app knows
 * about (`projectsDataStore.projects`, not a filtered view) and gets back the
 * rows to show.
 *
 * A query is matched in two tiers:
 *
 * 1. The app's own project search (`searchProjects` in projectSearch.ts):
 *    every whitespace-separated term is a case-insensitive substring of the
 *    name, a repo label, a branch, a PR or a note/review title, ranked by the
 *    best field each term was found in. The palette reuses it unchanged, so
 *    a query finds here what it finds in the sidebar and the landing grid,
 *    in the same order.
 * 2. A fuzzy tier for what tier 1 leaves out: every term is either a
 *    substring of the name or a repo label, or an in-order subsequence of one
 *    (`cmdk` finds "Command K search"). These always rank below tier 1.
 *
 * With no query the palette lists the most recently active projects.
 */

import type { Branch, Project, ProjectRepo } from '../../types';
import { highlightSegments, queryTerms, type HighlightSegment } from '../../shared/textSearch';
import { projectDisplayName } from '../../shared/utils';
import { searchProjects, type MatchReason, type SearchSnapshot } from '../projects/projectSearch';

/** The most rows the palette renders; `PaletteResults.total` says how many
 *  matched. A query narrows faster than a longer list scrolls. */
export const PALETTE_ROW_LIMIT = 50;

// ── Fuzzy matching ──
//
// A term matches a text fuzzily when its characters appear in the text in
// order, starting at the beginning of a word. Scoring favours runs of
// adjacent characters and characters that start a word, and charges for
// every character skipped, so `cmdk` finds "Command K search" while `ab`
// does not find "a very long name with b".

/** A matched character that directly follows the previous matched one. */
const FUZZY_ADJACENT = 4;
/** A matched character that starts a word. */
const FUZZY_WORD_START = 6;
/** Charged per character skipped between two matched ones. */
const FUZZY_SKIP = 1;
/** Shortest term matched fuzzily: one letter is a substring or nothing. */
const FUZZY_MIN_TERM = 2;

export interface FuzzyMatch {
  /** Higher is a tighter match; comparable between texts for one term. */
  score: number;
  /** Index in the text of each of the term's characters, ascending. */
  positions: number[];
}

const isWordChar = (ch: string) => /[\p{L}\p{N}]/u.test(ch);
const isUpper = (ch: string) => ch !== ch.toLowerCase();

/** Whether `text[i]` starts a word: after a separator (space, `/`, `-`,
 *  `_`, `.`) or at a lower-to-upper camelCase step. */
function startsWord(text: string, i: number): boolean {
  if (!isWordChar(text[i])) return false;
  if (i === 0) return true;
  const before = text[i - 1];
  if (!isWordChar(before)) return true;
  return isUpper(text[i]) && !isUpper(before);
}

/**
 * The best in-order match of `term` (lower-cased, as queryTerms returns it)
 * in `text`, or null when there is none good enough: the first character must
 * start a word, and the skipped characters must not outweigh the runs and
 * word starts the match earns (score below the term's length).
 *
 * Texts whose lower-casing changes their length (`İ`) are not matched
 * fuzzily: positions would no longer line up with `text`.
 */
export function fuzzyMatch(term: string, text: string): FuzzyMatch | null {
  const n = term.length;
  const lower = text.toLowerCase();
  const m = lower.length;
  if (n < FUZZY_MIN_TERM || n > m || m !== text.length) return null;

  // best[i * m + j]: the best score of term[0..i] with term[i] at text[j].
  // from[i * m + j]: where term[i - 1] sat in that match.
  const NONE = Number.NEGATIVE_INFINITY;
  const best = new Float64Array(n * m).fill(NONE);
  const from = new Int32Array(n * m).fill(-1);

  for (let j = 0; j < m; j++) {
    if (lower[j] === term[0] && startsWord(text, j)) best[j] = FUZZY_WORD_START;
  }
  for (let i = 1; i < n; i++) {
    const row = i * m;
    const prev = row - m;
    // The best earlier position to skip from, kept as score + SKIP * k so the
    // per-character charge needs no second loop: skipping from k to j costs
    // SKIP * (j - k - 1).
    let skipBest = NONE;
    let skipFrom = -1;
    for (let j = i; j < m; j++) {
      const k = j - 2;
      if (k >= 0 && best[prev + k] !== NONE) {
        const reach = best[prev + k] + FUZZY_SKIP * k;
        if (reach > skipBest) {
          skipBest = reach;
          skipFrom = k;
        }
      }
      if (lower[j] !== term[i]) continue;
      const adjacent = best[prev + j - 1] === NONE ? NONE : best[prev + j - 1] + FUZZY_ADJACENT;
      const skipped = skipBest === NONE ? NONE : skipBest - FUZZY_SKIP * (j - 1);
      if (adjacent === NONE && skipped === NONE) continue;
      const viaAdjacent = adjacent >= skipped;
      best[row + j] =
        (viaAdjacent ? adjacent : skipped) + (startsWord(text, j) ? FUZZY_WORD_START : 0);
      from[row + j] = viaAdjacent ? j - 1 : skipFrom;
    }
  }

  let end = -1;
  const last = (n - 1) * m;
  for (let j = 0; j < m; j++) {
    if (best[last + j] !== NONE && (end === -1 || best[last + j] > best[last + end])) end = j;
  }
  if (end === -1 || best[last + end] < n) return null;

  const positions = new Array<number>(n);
  for (let i = n - 1, j = end; i >= 0; j = from[i * m + j], i--) positions[i] = j;
  return { score: best[last + end], positions };
}

// ── Rows ──

/** One palette result. */
export interface PaletteRow {
  project: Project;
  /** The project's name, split into matched and unmatched runs. */
  name: HighlightSegment[];
  /** The repo path the row shows, likewise; null for a project without one. */
  repo: HighlightSegment[] | null;
  /** How many more repos the project has than the one shown. */
  moreRepos: number;
  /** Why the query found the project when neither its name nor its repos
   *  say (a branch, a PR, a note or review title). */
  reason: MatchReason | null;
}

export interface PaletteResults {
  rows: PaletteRow[];
  /** How many projects matched; more than `rows.length` past the limit. */
  total: number;
  /** The query's terms (queryTerms), for highlighting a row's reason. */
  terms: string[];
}

export interface PaletteSearchInput {
  /** Every project the app knows about, in any order. */
  projects: readonly Project[];
  query: string;
  /** Prepared search data (buildSearchSnapshot) for the same projects;
   *  without it only names are searched. */
  snapshot?: SearchSnapshot | null;
  /** Hydrated repos by project id; a project missing here falls back to its
   *  own `githubRepo`. */
  reposByProject?: ReadonlyMap<string, readonly ProjectRepo[]>;
  /** Hydrated branches by project id, for recency. */
  branchesByProject?: ReadonlyMap<string, readonly Branch[]>;
  /** The project on screen: listed last when there is no query, since
   *  opening it goes nowhere. */
  currentProjectId?: string | null;
  limit?: number;
}

/** A repo's path as the app writes it: `owner/name` or `owner/name/subpath`,
 *  a fork PR's head repo standing in for the base. */
function repoPath(githubRepo: string, subpath: string | null): string {
  return subpath ? `${githubRepo}/${subpath}` : githubRepo;
}

/** The project's repo paths, primary first. Before its repos hydrate, the
 *  project's own repo stands in, as on the grid card. */
export function projectRepoPaths(
  project: Project,
  repos: readonly ProjectRepo[] | undefined
): string[] {
  if (repos && repos.length > 0) {
    return [...repos]
      .sort((a, b) => Number(b.isPrimary) - Number(a.isPrimary))
      .map((repo) => repoPath(repo.headRepo ?? repo.githubRepo, repo.subpath));
  }
  return project.githubRepo ? [repoPath(project.githubRepo, project.subpath)] : [];
}

/**
 * When the project was last active: the latest change to the project itself
 * or to any of its branches (a rename, a workspace start, a move). Branches
 * that haven't hydrated yet simply don't count.
 */
export function projectActivityAt(project: Project, branches?: readonly Branch[]): number {
  let latest = Math.max(project.updatedAt, project.createdAt);
  for (const branch of branches ?? []) latest = Math.max(latest, branch.updatedAt);
  return latest;
}

/** `projects` most recently active first; ties keep their input order. */
export function byRecentActivity(
  projects: readonly Project[],
  branchesByProject?: ReadonlyMap<string, readonly Branch[]>
): Project[] {
  const activity = new Map(
    projects.map((p) => [p.id, projectActivityAt(p, branchesByProject?.get(p.id))])
  );
  return [...projects].sort((a, b) => activity.get(b.id)! - activity.get(a.id)!);
}

/** `text` split by a per-character mark. */
function segmentsFromMarks(text: string, marked: Uint8Array): HighlightSegment[] {
  const segments: HighlightSegment[] = [];
  let start = 0;
  for (let i = 1; i <= text.length; i++) {
    if (i === text.length || marked[i] !== marked[start]) {
      segments.push({ text: text.slice(start, i), match: marked[start] === 1 });
      start = i;
    }
  }
  return segments;
}

/**
 * `text` with every term highlighted: each occurrence of a term it holds as a
 * substring and, for a term it only matches fuzzily, the characters of that
 * match. A term found neither way leaves the text alone.
 */
export function fuzzyHighlightSegments(text: string, terms: readonly string[]): HighlightSegment[] {
  if (!text) return [];
  const lower = text.toLowerCase();
  // Substring highlighting copes with a lower-casing that changes the
  // length; fuzzy matching doesn't apply to such a text at all.
  if (lower.length !== text.length) return highlightSegments(text, terms);
  const marked = new Uint8Array(text.length);
  for (const term of terms) {
    if (!term) continue;
    if (lower.includes(term)) {
      for (let i = lower.indexOf(term); i !== -1; i = lower.indexOf(term, i + 1)) {
        marked.fill(1, i, i + term.length);
      }
      continue;
    }
    for (const position of fuzzyMatch(term, text)?.positions ?? []) marked[position] = 1;
  }
  return segmentsFromMarks(text, marked);
}

// What a term is worth in the fuzzy tier, by how it was found. A substring
// always outranks a fuzzy match of the same field, and the name outranks a
// repo path; within one kind the fuzzy score orders the matches (it stays
// well under the gap between kinds for any term a person types).
const TIER_NAME_SUBSTRING = 4000;
const TIER_REPO_SUBSTRING = 3000;
const TIER_NAME_FUZZY = 2000;
const TIER_REPO_FUZZY = 1000;

/**
 * The fuzzy tier's score for one project, or -1 when some term is found
 * neither as a substring nor fuzzily in the name or a repo path.
 */
function fuzzyScore(name: string, repoPaths: readonly string[], terms: readonly string[]): number {
  const lowerName = name.toLowerCase();
  let total = 0;
  for (const term of terms) {
    let best = -1;
    if (lowerName.includes(term)) {
      best = TIER_NAME_SUBSTRING;
    } else if (repoPaths.some((path) => path.toLowerCase().includes(term))) {
      best = TIER_REPO_SUBSTRING;
    } else {
      const inName = fuzzyMatch(term, name);
      if (inName) {
        best = TIER_NAME_FUZZY + inName.score;
      } else {
        for (const path of repoPaths) {
          const inRepo = fuzzyMatch(term, path);
          if (inRepo) best = Math.max(best, TIER_REPO_FUZZY + inRepo.score);
        }
      }
    }
    if (best === -1) return -1;
    total += best;
  }
  return total;
}

/** Whether `segments` highlight anything. */
function hasMatch(segments: readonly HighlightSegment[]): boolean {
  return segments.some((segment) => segment.match);
}

function buildRow(
  project: Project,
  repoPaths: readonly string[],
  terms: readonly string[],
  fuzzy: boolean,
  reason: MatchReason | null
): PaletteRow {
  const highlight = fuzzy ? fuzzyHighlightSegments : highlightSegments;
  const name = highlight(projectDisplayName(project), terms);
  // The repo the row shows is the first the query highlights, else the
  // primary, so a match on a secondary repo is visible in the row.
  let repo: HighlightSegment[] | null = null;
  for (const path of repoPaths) {
    const segments = highlight(path, terms);
    if (!repo) repo = segments;
    if (hasMatch(segments)) {
      repo = segments;
      break;
    }
  }
  return { project, name, repo, moreRepos: Math.max(0, repoPaths.length - 1), reason };
}

/**
 * The palette's rows for `query`: tier-1 matches in the app search's order
 * (ties most recently active first), then the fuzzy tier, best first. A blank
 * query lists every project most recently active first, the one on screen
 * last. Capped at `limit` rows either way.
 */
export function paletteResults({
  projects,
  query,
  snapshot = null,
  reposByProject,
  branchesByProject,
  currentProjectId = null,
  limit = PALETTE_ROW_LIMIT,
}: PaletteSearchInput): PaletteResults {
  const terms = queryTerms(query);
  const recent = byRecentActivity(projects, branchesByProject);
  const pathsOf = (project: Project) => projectRepoPaths(project, reposByProject?.get(project.id));

  if (terms.length === 0) {
    const current = recent.find((p) => p.id === currentProjectId);
    const ordered = current ? [...recent.filter((p) => p !== current), current] : recent;
    return {
      rows: ordered.slice(0, limit).map((p) => buildRow(p, pathsOf(p), terms, false, null)),
      total: ordered.length,
      terms,
    };
  }

  const exact = searchProjects(recent, query, snapshot);
  const found = new Set(exact.projects.map((p) => p.id));
  const fuzzy: Array<{ project: Project; paths: string[]; score: number }> = [];
  for (const project of recent) {
    if (found.has(project.id)) continue;
    const paths = pathsOf(project);
    const score = fuzzyScore(projectDisplayName(project), paths, terms);
    if (score >= 0) fuzzy.push({ project, paths, score });
  }
  // Array#sort is stable: equal scores keep the recency order.
  fuzzy.sort((a, b) => b.score - a.score);

  const rows: PaletteRow[] = [];
  for (const project of exact.projects) {
    if (rows.length >= limit) break;
    rows.push(
      buildRow(project, pathsOf(project), terms, false, exact.reasons.get(project.id) ?? null)
    );
  }
  for (const { project, paths } of fuzzy) {
    if (rows.length >= limit) break;
    rows.push(buildRow(project, paths, terms, true, null));
  }
  return { rows, total: exact.projects.length + fuzzy.length, terms };
}

// ── Selection ──

/**
 * The highlighted row after moving `delta` rows from `index` in a list of
 * `count`, wrapping at both ends. -1 (nothing highlighted) when the list is
 * empty; an out-of-range `index` moves from the nearest end.
 */
export function moveSelection(index: number, delta: number, count: number): number {
  if (count <= 0) return -1;
  const from = Math.min(Math.max(index, 0), count - 1);
  return (((from + delta) % count) + count) % count;
}
