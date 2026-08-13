/**
 * Deep link into Settings → Repos with a specific repo selected.
 *
 * Repo cards request this from their more menu, but the repos panel owns its
 * selection state and only resolves its entry list after an async load — so
 * the target is parked here and consumed by ActionsSettingsPanel once its
 * entries are ready.
 */

import { openSettings } from '../layout/navigation.svelte';

export interface RepoSettingsTarget {
  githubRepo: string;
  subpath: string;
}

let pendingTarget: RepoSettingsTarget | null = null;

/** Navigate to Settings → Repos and select the given repo once loaded. */
export function openRepoSettings(githubRepo: string, subpath: string | null | undefined): void {
  pendingTarget = { githubRepo, subpath: subpath ?? '' };
  openSettings('repo');
}

/** One-shot read of the most recently requested repo selection. */
export function consumeRepoSettingsTarget(): RepoSettingsTarget | null {
  const target = pendingTarget;
  pendingTarget = null;
  return target;
}
