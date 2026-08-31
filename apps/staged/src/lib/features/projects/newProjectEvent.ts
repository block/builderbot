import type { RepoSelection } from '../../shared/githubUrl';
import { projectFiltersStore, type RepoFilterRef } from './projectFilters.svelte';

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

/**
 * The repo to preselect when a window is narrowed to exactly one repo: that
 * repo. Zero or several active repo chips leave the form empty — there's no
 * single obvious answer to prefill. Status chips (Unread, Running) never reach
 * here, so one repo chip prefills whether or not they're also active.
 *
 * The filter selection is window-local (each webview has its own module graph,
 * so its own store instance), so this reads the filters of exactly the window
 * the new-project gesture happened in.
 */
export function repoSeedFromRepoFilters(filters: RepoFilterRef[]): RepoSelection | null {
  if (filters.length !== 1) return null;
  return { nameWithOwner: filters[0].repo, subpath: filters[0].subpath || undefined };
}

/**
 * The repo to preselect for a `staged:new-project` event: the one the event
 * carries, else the window's single active repo filter. A repo card names its
 * repo explicitly and so wins over the filter.
 */
export function repoSeedFromNewProjectEvent(event: Event): RepoSelection | null {
  const detail = (event as CustomEvent<NewProjectEventDetail | undefined>).detail;
  if (!detail?.githubRepo) return repoSeedFromRepoFilters(projectFiltersStore.activeRepoFilters);
  return { nameWithOwner: detail.githubRepo, subpath: detail.subpath || undefined };
}
