import type { RepoSelection } from '../../shared/githubUrl';

/**
 * Detail payload for the `staged:new-project` window event. Plain "new
 * project" triggers (⌘N, the top bar + button) dispatch it with no detail;
 * a repo card's new-project action includes its repo so the form opens with
 * that repo/subpath preselected.
 */
export interface NewProjectEventDetail {
  githubRepo?: string;
  subpath?: string;
}

/** Extract the repo to preselect from a `staged:new-project` event, if any. */
export function repoSeedFromNewProjectEvent(event: Event): RepoSelection | null {
  const detail = (event as CustomEvent<NewProjectEventDetail | undefined>).detail;
  if (!detail?.githubRepo) return null;
  return { nameWithOwner: detail.githubRepo, subpath: detail.subpath || undefined };
}
