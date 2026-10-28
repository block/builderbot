/**
 * Search rules for the projects sidebar's single "Search repos & projects"
 * field. The query itself is projectsListViewState.searchQuery, shared with
 * the landing-page grid, so the two surfaces always narrow together.
 *
 * Plain module (no runes, no store imports) like projectSearch.ts, so the
 * matching stays unit-testable. Projects reuse searchProjects; this file owns
 * the repo side and the shared header copy.
 */

import type { RepoHomeItem } from '../../types';

/** Trimmed, lower-cased query; empty when the field is blank. */
export function normalizeQuery(query: string): string {
  return query.trim().toLowerCase();
}

/**
 * Case-insensitive substring match against the repo's full path
 * (`githubRepo/subpath`) and its short name, so "builderbot", "apps/staged"
 * and "staged" all find the same card. An empty query returns the input
 * array unchanged.
 */
export function searchRepos(repos: RepoHomeItem[], query: string): RepoHomeItem[] {
  const q = normalizeQuery(query);
  if (!q) return repos;
  return repos.filter((r) => {
    const path = r.subpath ? `${r.githubRepo}/${r.subpath}` : r.githubRepo;
    return path.toLowerCase().includes(q) || r.shortName.toLowerCase().includes(q);
  });
}

/**
 * The repos the sidebar's repos segment shows: the pinned set in pin order,
 * narrowed by the query when there is one. Unpinned repos never appear here —
 * the segment is the pinned shelf, and All repos is the place to find the
 * rest — so a search can only ever hide pinned cards, not add others.
 * `homeRepos` is already pinned-first (store.list_repos_for_home), so the
 * pin order survives the filter.
 */
export function visibleSidebarRepos(homeRepos: RepoHomeItem[], query: string): RepoHomeItem[] {
  return searchRepos(
    homeRepos.filter((r) => r.pinned),
    query
  );
}

/** "1 result" / "n results" — the count a section header shows while searching. */
export function resultsLabel(count: number): string {
  return `${count} ${count === 1 ? 'result' : 'results'}`;
}
