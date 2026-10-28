// suggestedRepos.ts — pure helpers for SuggestedRepos.svelte.
//
// Kept out of the component so the "which attached repos does this project
// have" computation can be unit tested; vitest can't render Svelte components.

import type { ProjectRepo } from '../../types';

/** Stable key for a repo attachment: `owner/name` or `owner/name::subpath`. */
export function attachedRepoKey(repo: Pick<ProjectRepo, 'githubRepo' | 'subpath'>): string {
  const sub = repo.subpath?.trim();
  return sub ? `${repo.githubRepo}::${sub}` : repo.githubRepo;
}

/** Keys of the repos attached to `projectId`, drawn from an all-projects map. */
export function attachedRepoKeys(
  projectId: string,
  reposById: Map<string, ProjectRepo>
): Set<string> {
  const keys = new Set<string>();
  for (const repo of reposById.values()) {
    if (repo.projectId === projectId) keys.add(attachedRepoKey(repo));
  }
  return keys;
}

/**
 * Order-independent string form of a key set. Equal for equal sets, so a
 * reactive consumer keyed on it only re-runs when the set's contents change,
 * not when the map it was built from is reassigned.
 */
export function attachedRepoSignature(keys: Iterable<string>): string {
  // NUL can't appear in a GitHub `owner/name` or a filesystem subpath, so two
  // different key sets can never join to the same string.
  return [...keys].sort().join('\0');
}
