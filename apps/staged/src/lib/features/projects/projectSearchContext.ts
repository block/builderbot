/**
 * The SearchContext both project search surfaces (the landing grid and the
 * sidebar) match against, read from the shared stores. Call it inside a
 * `$derived` so the matches follow the stores: branches and repos as projects
 * hydrate, badge short names as they load, titles once the search index lands.
 */

import { projectsDataStore } from '../../stores/projectsData.svelte';
import { repoBadgeStore } from '../../stores/repoBadges.svelte';
import type { SearchContext } from './projectSearch';
import { projectSearchIndex } from './projectSearchIndex.svelte';

export function projectSearchContext(): SearchContext {
  return {
    branchesByProject: projectsDataStore.branchesByProject,
    reposByProject: projectsDataStore.reposByProject,
    repoShortName: (githubRepo, subpath) => repoBadgeStore.lookup(githubRepo, subpath)?.shortName,
    index: projectSearchIndex.index,
  };
}
