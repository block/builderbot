/**
 * Search rules for the projects sidebar's single "Search repos & projects"
 * field.
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
 * The repos the sidebar's repos segment shows. With no query it is the pinned
 * set, in pin order; with a query it is every home repo that matches, pinned
 * or not, so the one field is a way to reach any repo. `homeRepos` is already
 * pinned-first (store.list_repos_for_home), so matches keep that order.
 */
export function visibleSidebarRepos(homeRepos: RepoHomeItem[], query: string): RepoHomeItem[] {
  if (!normalizeQuery(query)) return homeRepos.filter((r) => r.pinned);
  return searchRepos(homeRepos, query);
}

/** "1 result" / "n results" — the count a section header shows while searching. */
export function resultsLabel(count: number): string {
  return `${count} ${count === 1 ? 'result' : 'results'}`;
}
