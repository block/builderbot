/**
 * What both project search surfaces (the landing grid and the sidebar)
 * match against, read from the shared stores.
 */

import { projectsDataStore } from '../../stores/projectsData.svelte';
import { repoBadgeStore } from '../../stores/repoBadges.svelte';
import { buildSearchSnapshot, type SearchContext, type SearchSnapshot } from './projectSearch';
import { projectSearchIndex } from './projectSearchIndex.svelte';

function projectSearchContext(): SearchContext {
  return {
    branchesByProject: projectsDataStore.branchesByProject,
    reposByProject: projectsDataStore.reposByProject,
    repoShortName: (githubRepo, subpath) => repoBadgeStore.lookup(githubRepo, subpath)?.shortName,
    index: projectSearchIndex.index,
  };
}

/**
 * A search snapshot source for one surface: call the returned function
 * inside a `$derived`, and only while a query is active, so nobody pays for
 * branch and PR updates while the field is empty. The snapshot follows the
 * stores — every project, branches and repos as they hydrate, badge short
 * names as they load, titles once the search index lands — and is the same
 * object again when an update moved nothing a search reads (a PR status
 * poll, an index refetch with the same titles), so the results stay put.
 */
export function projectSearchSnapshots(): () => SearchSnapshot {
  let previous: SearchSnapshot | null = null;
  return () =>
    (previous = buildSearchSnapshot(projectsDataStore.projects, projectSearchContext(), previous));
}
