/**
 * The distinguishing part of a repo path.
 *
 * `RepoLabel.svelte` renders this segment at full contrast with everything
 * before it muted; the per-window title (`features/layout/windowTitle.ts`)
 * renders it *alone*, since macOS Window-menu items truncate and
 * `block/builderbot/apps/staged` would be mostly wasted width. Shared so the
 * chip and the title can't disagree about which segment matters.
 */

export interface RepoPathRef {
  repo: string;
  subpath?: string | null;
}

/**
 * The subpath when there is one, otherwise the text after the repo's final
 * `/`. A multi-segment subpath stays whole (`apps/staged`, not `staged`) — the
 * subpath is one unit, and this is the rule the repo chips have always drawn.
 */
export function repoEmphasis({ repo, subpath }: RepoPathRef): string {
  if (subpath) return subpath;
  const idx = repo.lastIndexOf('/');
  return idx >= 0 ? repo.slice(idx + 1) : repo;
}
