/**
 * The window's project sort: the ordered keys the grid, the sidebar and
 * every board are sorted by (projectOrder).
 *
 * Frontend-only, loaded once from initPreferences and saved on change, like
 * boardViewPrefs. It deliberately doesn't follow other windows (no
 * syncedStoreValue): sorting one window leaves the others as they are. The
 * file holds the last sort written, and a new window starts from it.
 *
 * The sort is held as stored: it may name a tag group since deleted. Readers
 * resolve it against the catalog (resolveProjectSort), and the edit helpers
 * in projectSort.ts prune it, so the next write drops the dead key and
 * nothing is written just because the catalog changed.
 */

import { getStoreValue, setStoreValue } from '../../shared/persistentStore';
import { normalizeProjectSort, sortKeyId, type ProjectSort } from './projectSort';

export const PROJECT_SORT_STORE_KEY = 'projects-sort';

export const projectSortPrefs = $state({
  /** Empty is the order the projects come in: newest first. */
  sort: [] as ProjectSort,
});

/** Read the stored sort. Called once at startup, before the first paint. */
export async function loadProjectSortPrefs(): Promise<void> {
  projectSortPrefs.sort = normalizeProjectSort(
    await getStoreValue<unknown>(PROJECT_SORT_STORE_KEY)
  );
}

function sameSort(a: ProjectSort, b: ProjectSort): boolean {
  return (
    a.length === b.length &&
    a.every((key, i) => sortKeyId(key) === sortKeyId(b[i]) && key.direction === b[i].direction)
  );
}

/**
 * Sort this window by `sort` and remember it. A sort with the same keys in
 * the same order and directions as the current one changes nothing and
 * writes nothing, so an edit helper's "nothing to do" result can be passed
 * straight in.
 */
export function setProjectSort(sort: ProjectSort): void {
  const next = normalizeProjectSort(sort);
  if (sameSort(next, projectSortPrefs.sort)) return;
  projectSortPrefs.sort = next;
  setStoreValue(PROJECT_SORT_STORE_KEY, next).catch((e) => {
    console.error(`[projectSortPrefs] Failed to save ${PROJECT_SORT_STORE_KEY}:`, e);
  });
}
