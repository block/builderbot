/**
 * Search rules for the projects sidebar's single "Search repos & projects"
 * field. The query itself is projectsListViewState.searchQuery, shared with
 * the landing-page grid, so the two surfaces always narrow together.
 *
 * Plain module (no runes, no store imports) like projectSearch.ts, so the
 * matching stays unit-testable. Projects reuse searchProjects and its
 * SearchSnapshot (branches, PRs, repo labels, titles); this file owns the
 * repo side, the match sets the sidebar hides rows by, and the shared header
 * copy.
 *
 * The sidebar doesn't render the narrowed lists. Each segment renders its
 * whole list and hides what the query rules out (the match sets below): a
 * keyed `{#each}` over the narrowed list destroys every row a keystroke
 * filters out and rebuilds it on the next backspace, and a row is costly to
 * build — a context menu per project, and per repo card an actions IPC wave
 * plus three backend event subscriptions. Hiding turns a keystroke into a
 * `hidden` flip on rows that already exist, and the project rows' ranking
 * into a CSS `order` on each (SidebarProjectMatches.rank). Repo cards keep
 * their pin order.
 */

import type { Project, RepoHomeItem } from '../../types';
import { matchesAllTerms, queryTerms } from '../../shared/textSearch';
import { searchProjects, type MatchReason, type SearchSnapshot } from './projectSearch';
import { repoKey } from './repoReorder';

/**
 * Case-insensitive match against the repo's full path (`githubRepo/subpath`)
 * and its short name, so "builderbot", "apps/staged" and "staged" all find
 * the same card. Every whitespace-separated term of the query must appear in
 * one of the two, in any order ("staged block" finds block/builderbot/apps/
 * staged). An empty query returns the input array unchanged.
 */
export function searchRepos(repos: RepoHomeItem[], query: string): RepoHomeItem[] {
  const terms = queryTerms(query);
  if (terms.length === 0) return repos;
  return repos.filter((r) => {
    const path = r.subpath ? `${r.githubRepo}/${r.subpath}` : r.githubRepo;
    return matchesAllTerms(terms, path, r.shortName);
  });
}

/**
 * The repos the sidebar's repos segment renders: the pinned set, in pin
 * order. Unpinned repos never appear here — the segment is the pinned shelf,
 * and All repos is the place to find the rest — so a search can only ever
 * hide pinned cards, not add others. `homeRepos` is already pinned-first
 * (store.list_repos_for_home), so the filter keeps the pin order.
 */
export function sidebarPinnedRepos(homeRepos: RepoHomeItem[]): RepoHomeItem[] {
  return homeRepos.filter((r) => r.pinned);
}

/**
 * Keys (repoKey) of the repos the query keeps, or null when the query is
 * blank and every card shows.
 */
export function matchingRepoKeys(repos: RepoHomeItem[], query: string): Set<string> | null {
  if (queryTerms(query).length === 0) return null;
  return new Set(searchRepos(repos, query).map(repoKey));
}

export interface SidebarProjectMatches {
  /** Each kept project row's place on screen, by id (searchProjects' rank):
   *  rows it doesn't hold are hidden, the rest laid out in this order. Null
   *  when the query is blank and every row shows in its own order. */
  rank: ReadonlyMap<string, number> | null;
  /** Why each kept project matched, when its name and repos don't say. */
  reasons: ReadonlyMap<string, MatchReason>;
}

/** The project rows the query keeps (searchProjects' rule), their rank, and why. */
export function sidebarProjectMatches(
  projects: Project[],
  query: string,
  snapshot?: SearchSnapshot | null
): SidebarProjectMatches {
  const { rank, reasons } = searchProjects(projects, query, snapshot);
  return { rank, reasons };
}

/** "1 result" / "n results" — the count a section header shows while searching. */
export function resultsLabel(count: number): string {
  return `${count} ${count === 1 ? 'result' : 'results'}`;
}
