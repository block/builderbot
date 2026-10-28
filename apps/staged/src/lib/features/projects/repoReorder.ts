/**
 * Pure helpers for PinnedRepoCards' drag-to-reorder.
 *
 * A drag spans two renders, so the card indices captured at dragstart and
 * drop can describe different lists if `repos` changes mid-drag (an unpin
 * from a card's ⋮ menu, a `repos-changed` echo from another window). Both
 * ends are therefore recorded as repo keys and resolved against the list
 * that exists at drop time.
 */

import type { RepoHomeItem } from '../../types';

/** Identity of a repo card: `githubRepo` + `subpath`, also the `{#each}` key. */
export function repoKey(repo: Pick<RepoHomeItem, 'githubRepo' | 'subpath'>): string {
  return `${repo.githubRepo}\t${repo.subpath}`;
}

/**
 * Moves the repo with `sourceKey` to the position of the repo with
 * `targetKey`, returning the new order. Returns null — nothing to apply or
 * persist — when either key is missing from `repos` or they are the same.
 */
export function moveRepo(
  repos: RepoHomeItem[],
  sourceKey: string,
  targetKey: string
): RepoHomeItem[] | null {
  if (sourceKey === targetKey) return null;
  const from = repos.findIndex((r) => repoKey(r) === sourceKey);
  const to = repos.findIndex((r) => repoKey(r) === targetKey);
  if (from === -1 || to === -1) return null;
  const items = [...repos];
  const [moved] = items.splice(from, 1);
  items.splice(to, 0, moved);
  return items;
}
