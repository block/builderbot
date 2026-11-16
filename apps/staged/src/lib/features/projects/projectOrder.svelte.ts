/**
 * The one order every project list is in: the live projects run through the
 * window's sort (projectSortPrefs).
 *
 * The grid, the sidebar, the home board and the pinned boards all render
 * from `sortedProjects`, directly or through projectFiltersStore's
 * `filteredProjects`, which filters it and keeps its order. Next and
 * previous project walk it too. None of them sorts, so they cannot disagree.
 * A board needs no rule of its own: bucketing a sorted list into columns
 * keeps each column sorted, and a key for the group the board is columned by
 * ties inside every column, so the next key decides.
 *
 * With no sort, which is the default, `sortedProjects` is `liveProjects`
 * itself: the same array, in the backend's newest-first order, and nothing
 * here reads the tags or the activity, so nothing downstream recomputes.
 *
 * With one, the order follows what its keys read, live:
 * - tag positions, from pendingCatalogMoves, so a column or settings reorder
 *   re-sorts at once, before its write lands
 * - each project's tags, pending picks included
 * - last activity (projectActivity), loaded only while a last-updated key is
 *   in the sort, and held back while a pointer is down
 *
 * A change that moves no project hands back the previous array (see
 * sortProjects), so an activity refresh or a recoloured tag re-renders
 * nothing.
 */

import { untrack } from 'svelte';
import type { Project } from '../../types';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { tagsStore } from '../../stores/tags.svelte';
import { pendingCatalogMoves } from '../tags/pendingCatalogMoves.svelte';
import { projectActivity } from './projectActivity.svelte';
import { resolveProjectSort, sortProjects, sortUsesActivity } from './projectSort';
import { projectSortPrefs } from './projectSortPrefs.svelte';

class ProjectOrderStore {
  /** The last sorted array, handed back when a recompute moves nothing. */
  private previous: Project[] | null = null;

  /**
   * The live projects in the window's sort order: what every list renders
   * from. `liveProjects` itself while the sort has no keys.
   */
  readonly sortedProjects: Project[] = $derived.by(() => {
    const projects = projectsDataStore.liveProjects;
    const stored = projectSortPrefs.sort;
    // Before anything else is read: an unsorted window follows nothing but
    // the list and the sort itself.
    if (stored.length === 0) return (this.previous = projects);
    const catalog = pendingCatalogMoves.catalog;
    const sort = resolveProjectSort(stored, catalog);
    if (sort.length === 0) return (this.previous = projects);
    return (this.previous = sortProjects(
      projects,
      sort,
      {
        catalog,
        projectTagIds: tagsStore.projectTagIds,
        // Read only by a last-updated key, so other sorts don't follow it.
        activity: sortUsesActivity(sort) ? projectActivity.activity : null,
      },
      this.previous
    ));
  });

  constructor() {
    // The activity read is lazy: asked for when the sort gains a
    // last-updated key (at startup, from the stored sort, or by an edit),
    // which is the one time nothing else would fetch it. After that the
    // activity store follows the change feed itself. A root effect, since
    // this store outlives every component.
    $effect.root(() => {
      $effect(() => {
        if (!sortUsesActivity(projectSortPrefs.sort)) return;
        untrack(() => projectActivity.ensureLoaded());
      });
    });
  }
}

export const projectOrderStore = new ProjectOrderStore();
