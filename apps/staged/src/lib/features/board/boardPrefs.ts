/**
 * The home page's view preferences (Grid or Board, which group the board is
 * by, and each board's collapsed and hidden columns), as pure rules: reading
 * them from the store tolerantly, resolving them against the tag catalog, and
 * pruning what the catalog no longer has.
 *
 * Unknown ids are ignored on read and pruned on the next write, so a deleted
 * group or tag never leaves a column hidden for good, and nothing is written
 * just because the catalog changed.
 */

import type { TagGroup } from '../../types';
import { NO_TAG_MENU_VALUE, type TagCatalog } from '../tags/tagState';

export type ProjectsViewMode = 'grid' | 'board';

/** A board column's key for "No ‹group›": projects without a tag in the group. */
export const NO_TAG_COLUMN_KEY = NO_TAG_MENU_VALUE;

/** One board's column state. A key is a tag id or NO_TAG_COLUMN_KEY. */
export interface BoardColumnPrefs {
  collapsed: string[];
  hidden: string[];
}

/** Every board's column state, keyed by group id. */
export type BoardColumnsPrefs = Record<string, BoardColumnPrefs>;

export interface ResolvedColumnPrefs {
  collapsed: ReadonlySet<string>;
  hidden: ReadonlySet<string>;
}

export function normalizeViewMode(value: unknown): ProjectsViewMode {
  return value === 'board' ? 'board' : 'grid';
}

export function normalizeBoardGroupId(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

function stringList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.filter((item): item is string => typeof item === 'string'))];
}

/** The stored column state, reshaped: malformed entries are dropped, ids kept as they are. */
export function normalizeBoardColumns(value: unknown): BoardColumnsPrefs {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const columns: BoardColumnsPrefs = {};
  for (const [groupId, entry] of Object.entries(value)) {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) continue;
    const { collapsed, hidden } = entry as Record<string, unknown>;
    columns[groupId] = { collapsed: stringList(collapsed), hidden: stringList(hidden) };
  }
  return columns;
}

/**
 * The group the board is by: the chosen one while it exists, else the icon
 * group, else the first group. Null only when there are no groups.
 */
export function resolveBoardGroup(
  groupId: string | null,
  catalog: Pick<TagCatalog, 'groups' | 'groupsById' | 'iconGroup'>
): TagGroup | null {
  return (
    (groupId ? catalog.groupsById.get(groupId) : undefined) ??
    catalog.iconGroup ??
    catalog.groups[0] ??
    null
  );
}

/** Whether `key` is still one of `groupId`'s columns. */
function isColumnOf(key: string, groupId: string, catalog: Pick<TagCatalog, 'tagsById'>): boolean {
  return key === NO_TAG_COLUMN_KEY || catalog.tagsById.get(key)?.groupId === groupId;
}

/** A board's collapsed and hidden columns, minus keys that are no longer its columns. */
export function columnPrefsFor(
  columns: BoardColumnsPrefs,
  groupId: string,
  catalog: Pick<TagCatalog, 'tagsById'>
): ResolvedColumnPrefs {
  const entry = columns[groupId];
  const known = (keys: string[] | undefined) =>
    new Set((keys ?? []).filter((key) => isColumnOf(key, groupId, catalog)));
  return { collapsed: known(entry?.collapsed), hidden: known(entry?.hidden) };
}

/**
 * The column state with everything the catalog no longer has dropped: boards
 * for deleted groups, and keys for tags deleted or moved out of the group.
 * Boards left with nothing collapsed or hidden go too.
 */
export function pruneBoardColumns(
  columns: BoardColumnsPrefs,
  catalog: Pick<TagCatalog, 'groupsById' | 'tagsById'>
): BoardColumnsPrefs {
  const pruned: BoardColumnsPrefs = {};
  for (const [groupId, entry] of Object.entries(columns)) {
    if (!catalog.groupsById.has(groupId)) continue;
    const { collapsed, hidden } = columnPrefsFor(columns, groupId, catalog);
    if (collapsed.size === 0 && hidden.size === 0) continue;
    // Keep the stored order, so a save doesn't reshuffle the file.
    pruned[groupId] = {
      collapsed: entry.collapsed.filter((key) => collapsed.has(key)),
      hidden: entry.hidden.filter((key) => hidden.has(key)),
    };
  }
  return pruned;
}

/**
 * The column state after turning `flag` on or off for one of a board's
 * columns, pruned against the catalog.
 */
export function withColumnFlag(
  columns: BoardColumnsPrefs,
  groupId: string,
  key: string,
  flag: keyof BoardColumnPrefs,
  on: boolean,
  catalog: Pick<TagCatalog, 'groupsById' | 'tagsById'>
): BoardColumnsPrefs {
  const entry = columns[groupId] ?? { collapsed: [], hidden: [] };
  const keys = entry[flag].filter((existing) => existing !== key);
  if (on) keys.push(key);
  return pruneBoardColumns({ ...columns, [groupId]: { ...entry, [flag]: keys } }, catalog);
}
