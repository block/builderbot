/**
 * How the project lists are ordered, as pure rules: the sort a window holds,
 * reading it from the store tolerantly, resolving it against the tag catalog,
 * editing it, describing it, and ordering projects by it.
 *
 * A sort is an ordered list of keys, the first the most significant. Each
 * later key only breaks the ties the ones before it leave, and what is still
 * tied keeps the order the projects came in, which is the backend's newest
 * first. An empty list is that order untouched: sortProjects hands back the
 * array it was given, so a window that never sorts recomputes nothing.
 *
 * A key naming a group the catalog no longer has is ignored on read and
 * pruned on the next write, as boardPrefs does, so a deleted group never
 * leaves a dead key behind and nothing is written just because the catalog
 * changed.
 */

import type { Project } from '../../types';
import { groupTag, type TagCatalog } from '../tags/tagState';

export type SortDirection = 'asc' | 'desc';

export type ProjectSortKey =
  /** By `createdAt`. */
  | { kind: 'created'; direction: SortDirection }
  /** By last activity (see lastUpdatedAt). */
  | { kind: 'updated'; direction: SortDirection }
  /** By the project's tag in a group, in the group's tag order. */
  | { kind: 'group'; groupId: string; direction: SortDirection };

export type ProjectSortKind = ProjectSortKey['kind'];

/** Ordered, first key most significant. Empty means the order the projects come in. */
export type ProjectSort = ProjectSortKey[];

// =============================================================================
// Keys
// =============================================================================

/**
 * What makes a key the same key whichever way it points: its kind, and for a
 * group key its group. A sort holds each id at most once, so it also names a
 * key to the edit helpers and keys a rendered list.
 */
export function sortKeyId(key: ProjectSortKey): string {
  return key.kind === 'group' ? `group:${key.groupId}` : key.kind;
}

/**
 * The direction a key starts in: newest first, most recently updated first,
 * and a group's own tag order (a board's columns, left to right).
 */
export function defaultSortDirection(kind: ProjectSortKind): SortDirection {
  return kind === 'group' ? 'asc' : 'desc';
}

function isDirection(value: unknown): value is SortDirection {
  return value === 'asc' || value === 'desc';
}

/** One stored entry as a key, or null when it isn't one. Extra fields are dropped. */
function normalizeSortKey(value: unknown): ProjectSortKey | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const { kind, direction, groupId } = value as Record<string, unknown>;
  if (!isDirection(direction)) return null;
  if (kind === 'created' || kind === 'updated') return { kind, direction };
  if (kind === 'group' && typeof groupId === 'string' && groupId.length > 0) {
    return { kind, groupId, direction };
  }
  return null;
}

/**
 * The stored sort, reshaped: malformed entries and unknown kinds are dropped,
 * and of two entries for one key the first, more significant one is kept.
 * Group ids are kept as they are, known to the catalog or not.
 */
export function normalizeProjectSort(value: unknown): ProjectSort {
  if (!Array.isArray(value)) return [];
  const sort: ProjectSort = [];
  const seen = new Set<string>();
  for (const entry of value) {
    const key = normalizeSortKey(entry);
    if (!key || seen.has(sortKeyId(key))) continue;
    seen.add(sortKeyId(key));
    sort.push(key);
  }
  return sort;
}

function isKnownKey(key: ProjectSortKey, catalog: Pick<TagCatalog, 'groupsById'>): boolean {
  return key.kind !== 'group' || catalog.groupsById.has(key.groupId);
}

/**
 * The sort minus keys for groups the catalog no longer has: what the sort
 * means now, and what the next write stores. `sort` itself comes back when
 * every key is known, so a catalog change that drops nothing changes nothing.
 */
export function resolveProjectSort(
  sort: ProjectSort,
  catalog: Pick<TagCatalog, 'groupsById'>
): ProjectSort {
  return sort.every((key) => isKnownKey(key, catalog))
    ? sort
    : sort.filter((key) => isKnownKey(key, catalog));
}

/**
 * Whether the sort has a last-updated key, which is the only key that needs
 * the per-project activity read. Never a group key, so it holds for the
 * stored sort and the resolved one alike.
 */
export function sortUsesActivity(sort: ProjectSort): boolean {
  return sort.some((key) => key.kind === 'updated');
}

// =============================================================================
// Edits
// =============================================================================
//
// Each takes the sort as stored and returns the resolved sort with the edit
// applied, so every write prunes. An edit that changes nothing (a key that
// is already there, or isn't, or can't move further) returns the resolved
// sort as it stands.

/**
 * The keys the sort could still take, each in its default direction: created,
 * last updated, then every group in catalog order, minus the ones in use.
 */
export function addableSortKeys(
  sort: ProjectSort,
  catalog: Pick<TagCatalog, 'groups'>
): ProjectSortKey[] {
  const used = new Set(sort.map(sortKeyId));
  const all: ProjectSortKey[] = [
    { kind: 'created', direction: defaultSortDirection('created') },
    { kind: 'updated', direction: defaultSortDirection('updated') },
    ...catalog.groups.map((group) => ({
      kind: 'group' as const,
      groupId: group.id,
      direction: defaultSortDirection('group'),
    })),
  ];
  return all.filter((key) => !used.has(sortKeyId(key)));
}

/** The sort with `key` added last, as the least significant. */
export function withSortKeyAdded(
  sort: ProjectSort,
  key: ProjectSortKey,
  catalog: Pick<TagCatalog, 'groupsById'>
): ProjectSort {
  const known = resolveProjectSort(sort, catalog);
  const id = sortKeyId(key);
  if (!isKnownKey(key, catalog) || known.some((existing) => sortKeyId(existing) === id)) {
    return known;
  }
  return [...known, key];
}

/** The sort without the key `id` names. */
export function withoutSortKey(
  sort: ProjectSort,
  id: string,
  catalog: Pick<TagCatalog, 'groupsById'>
): ProjectSort {
  const known = resolveProjectSort(sort, catalog);
  return known.some((key) => sortKeyId(key) === id)
    ? known.filter((key) => sortKeyId(key) !== id)
    : known;
}

/**
 * The sort with the key `id` names one place more (`-1`) or less (`1`)
 * significant, among the keys the catalog still has.
 */
export function withSortKeyMoved(
  sort: ProjectSort,
  id: string,
  step: -1 | 1,
  catalog: Pick<TagCatalog, 'groupsById'>
): ProjectSort {
  const known = resolveProjectSort(sort, catalog);
  const from = known.findIndex((key) => sortKeyId(key) === id);
  const to = from + step;
  if (from === -1 || to < 0 || to >= known.length) return known;
  const moved = [...known];
  [moved[from], moved[to]] = [moved[to], moved[from]];
  return moved;
}

/** The sort with the key `id` names pointing in `direction`. */
export function withSortKeyDirection(
  sort: ProjectSort,
  id: string,
  direction: SortDirection,
  catalog: Pick<TagCatalog, 'groupsById'>
): ProjectSort {
  const known = resolveProjectSort(sort, catalog);
  const current = known.find((key) => sortKeyId(key) === id);
  if (!current || current.direction === direction) return known;
  return known.map((key) => (key === current ? { ...key, direction } : key));
}

// =============================================================================
// Describing
// =============================================================================

/** The sort's name when it has no keys: the order the projects come in. */
export const DEFAULT_SORT_SUMMARY = 'Newest';

/**
 * A key's name: "Created", "Last updated", or its group's. Empty for a group
 * the catalog no longer has, which a resolved sort never holds.
 */
export function sortKeyLabel(key: ProjectSortKey, catalog: Pick<TagCatalog, 'groupsById'>): string {
  if (key.kind === 'created') return 'Created';
  if (key.kind === 'updated') return 'Last updated';
  return catalog.groupsById.get(key.groupId)?.name ?? '';
}

/**
 * What a direction means for a key of `kind`, as its menu words it: newest or
 * oldest first, most or least recent first, and for a group its tag order or
 * the reverse.
 */
export function sortDirectionLabel(kind: ProjectSortKind, direction: SortDirection): string {
  if (kind === 'created') return direction === 'desc' ? 'Newest first' : 'Oldest first';
  if (kind === 'updated') return direction === 'desc' ? 'Most recent first' : 'Least recent first';
  return direction === 'asc' ? 'Tag order' : 'Reverse tag order';
}

/** Both directions for a key of `kind`, the one it starts in first. */
export function sortDirections(kind: ProjectSortKind): [SortDirection, SortDirection] {
  return defaultSortDirection(kind) === 'asc' ? ['asc', 'desc'] : ['desc', 'asc'];
}

/**
 * The sort in a few words, for the control that opens its menu: its keys'
 * names in order ("Priority, Last updated"), or "Newest" with none.
 */
export function sortSummary(sort: ProjectSort, catalog: Pick<TagCatalog, 'groupsById'>): string {
  const known = resolveProjectSort(sort, catalog);
  if (known.length === 0) return DEFAULT_SORT_SUMMARY;
  return known.map((key) => sortKeyLabel(key, catalog)).join(', ');
}

/**
 * Whether `key` does nothing on a board by `boardGroupId`: every card in a
 * column has the same tag in that group, or none, so the key ties for the
 * whole column and the next one decides. It stays in the sort, since it
 * still orders the grid and the sidebar.
 */
export function isInertOnBoard(key: ProjectSortKey, boardGroupId: string | null): boolean {
  return key.kind === 'group' && key.groupId === boardGroupId;
}

// =============================================================================
// Sorting
// =============================================================================

/** What the keys read beyond the projects themselves. */
export interface ProjectSortContext {
  /** The tag catalog a group key takes its tag order from. */
  catalog: Pick<TagCatalog, 'groupsById' | 'tagsById' | 'tagsByGroup'>;
  /** Each project's tag ids. */
  projectTagIds: ReadonlyMap<string, ReadonlySet<string>>;
  /**
   * When each project last saw activity, for those that have any. Without
   * it, last updated is the creation time.
   */
  activity?: ReadonlyMap<string, number> | null;
}

/**
 * When a project was last updated: its latest activity, and never earlier
 * than its creation, so a new project with nothing in it counts as just
 * updated.
 */
export function lastUpdatedAt(
  project: Pick<Project, 'id' | 'createdAt'>,
  activity: ReadonlyMap<string, number> | null | undefined
): number {
  return Math.max(project.createdAt, activity?.get(project.id) ?? project.createdAt);
}

/** Sorts last whichever way its key points. */
const MISSING = Number.POSITIVE_INFINITY;

/**
 * What one key compares, as a number where lower sorts first: the key's
 * value, negated when it points down. A project with no tag in a group key's
 * group has no value and sorts last in both directions, so flipping Priority
 * leads with the lowest priority rather than a block of untagged projects.
 */
function keyValues(key: ProjectSortKey, ctx: ProjectSortContext): (project: Project) => number {
  const sign = key.direction === 'asc' ? 1 : -1;
  if (key.kind === 'created') return (project) => sign * project.createdAt;
  if (key.kind === 'updated') return (project) => sign * lastUpdatedAt(project, ctx.activity);

  const { groupId } = key;
  // A tag's place in the group's order: its index, not its `position`, so
  // tags that tie on position still sort as the board lays out their columns.
  const rank = new Map((ctx.catalog.tagsByGroup.get(groupId) ?? []).map((tag, i) => [tag.id, i]));
  return (project) => {
    const tag = groupTag(ctx.projectTagIds.get(project.id), groupId, ctx.catalog.tagsById);
    const place = tag ? rank.get(tag.id) : undefined;
    return place === undefined ? MISSING : sign * place;
  };
}

function sameProjects(a: readonly Project[], b: readonly Project[]): boolean {
  return a.length === b.length && a.every((project, i) => project === b[i]);
}

/**
 * `projects` in the sort's order. With no keys (or none the catalog still
 * has) that is `projects` itself, the same array.
 *
 * Pass the array this returned last time as `previous`: when the result
 * holds the same project objects in the same order, `previous` itself comes
 * back, so a `$derived` over it doesn't invalidate every list when, say, an
 * activity refresh moves timestamps without moving a project.
 */
export function sortProjects(
  projects: Project[],
  sort: ProjectSort,
  ctx: ProjectSortContext,
  previous?: Project[] | null
): Project[] {
  const values = resolveProjectSort(sort, ctx.catalog).map((key) => keyValues(key, ctx));
  if (values.length === 0) return projects;

  const rows = projects.map((project, index) => ({
    project,
    index,
    values: values.map((valueOf) => valueOf(project)),
  }));
  rows.sort((a, b) => {
    for (let k = 0; k < values.length; k++) {
      // Compared, not subtracted: two missing values are a tie, not NaN.
      if (a.values[k] < b.values[k]) return -1;
      if (a.values[k] > b.values[k]) return 1;
    }
    return a.index - b.index;
  });
  const sorted = rows.map((row) => row.project);
  return previous && sameProjects(sorted, previous) ? previous : sorted;
}
