/**
 * What a repo is called where its full path would crowd the line: the short
 * name and hue of its badge.
 *
 * A badge belongs to one (githubRepo, subpath) pair, so a monorepo's subpath
 * has a short name of its own (`block/builderbot` + `apps/staged` → "staged")
 * rather than a shortened path. The backend keeps short names unique across
 * every badge (two repos that would both be "goose" get "goose" and "goose2",
 * and a rename onto a taken name is refused), so nothing here disambiguates:
 * a short name already stands for exactly one repo.
 *
 * Shared by the sidebar (project rows, the Filter menu), the project grid's
 * cards and the board's, and the filter chips above both, so the surfaces
 * can't disagree about what a repo's short name is.
 */

import { repoBadgeStore } from '../stores/repoBadges.svelte';

export interface RepoRef {
  githubRepo: string;
  subpath?: string | null;
}

export interface RepoShortLabel {
  shortName: string;
  hue: number;
}

/**
 * The repo's short label, or undefined while it has no badge: one that hasn't
 * loaded or been generated yet, or a fork named by its head repo (badges are
 * keyed by `githubRepo`). Callers decide what to show then; there is no
 * second, derived short name to fall back on.
 *
 * Reads the badge store, so inside a `$derived` or a template it follows
 * badges as they load and as they're renamed.
 */
export function repoShortLabel(repo: RepoRef): RepoShortLabel | undefined {
  const badge = repoBadgeStore.lookup(repo.githubRepo, repo.subpath);
  return badge ? { shortName: badge.shortName, hue: badge.hue } : undefined;
}

/**
 * The repo's full path, `org/repo` or `org/repo/subpath`: what a short label's
 * tooltip says, since the label itself no longer does.
 */
export function repoFullName(repo: RepoRef): string {
  return repo.subpath ? `${repo.githubRepo}/${repo.subpath}` : repo.githubRepo;
}
