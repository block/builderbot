/**
 * Pure tag-state logic behind tagsStore: indexing the `get_tag_state`
 * snapshot, reconciling a fetched one with the one shown, layering pending
 * assignment writes (intents) over it, and the per-project lookups every tag
 * surface reads.
 *
 * Kept free of runes so the rules are unit-testable; the store only holds the
 * snapshot and the intent overlay in `$state` and derives through these.
 */

import type { ProjectTag, Tag, TagGroup, TagState } from '../../types';

export const EMPTY_TAG_STATE: TagState = { groups: [], tags: [], assignments: [] };

/**
 * Radio value for "no tag in this group" in the tag menus, and the board's
 * column key for it. Tag ids are slugs or UUIDs, so it can't clash.
 */
export const NO_TAG_MENU_VALUE = '__none__';

// =============================================================================
// Catalog
// =============================================================================

export interface TagCatalog {
  /** Every group, in position order. */
  groups: TagGroup[];
  groupsById: ReadonlyMap<string, TagGroup>;
  tagsById: ReadonlyMap<string, Tag>;
  /**
   * Each group's tags in position order, keyed by group id; `null` holds the
   * ungrouped tags. Every group has an entry, empty or not.
   */
  tagsByGroup: ReadonlyMap<string | null, Tag[]>;
  /** The group whose tag stands in for a project's computed status icon. */
  iconGroup: TagGroup | null;
  /** The groups whose boards are pinned to the sidebar, in position order. */
  sidebarGroups: TagGroup[];
}

/**
 * Catalog order: by position, then creation, then id (the backend's
 * `ORDER BY`). The backend keeps positions dense, so the tie-breaks only
 * decide the order of rows that broke that rule. Ids compare by code unit,
 * which is how SQLite orders them, not by locale.
 */
export function byPosition<T extends { id: string; position: number; createdAt: number }>(
  a: T,
  b: T
): number {
  return (
    a.position - b.position || a.createdAt - b.createdAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)
  );
}

/** Longest tag or group name, in characters (the backend rejects longer). */
export const MAX_TAG_NAME_CHARS = 64;

/**
 * The key tag and group names are unique by within their scope: SQLite's
 * `NOCASE`, which folds ASCII letters only, so "Infra" and "infra" collide
 * but "É" and "é" don't.
 */
export function tagNameKey(name: string): string {
  return name.trim().replace(/[A-Z]/g, (c) => c.toLowerCase());
}

/** `base` cut so that `base + suffix` fits the name limit. */
function fitName(base: string, suffix: string): string {
  const room = Math.max(0, MAX_TAG_NAME_CHARS - [...suffix].length);
  return `${[...base].slice(0, room).join('').trimEnd()}${suffix}`;
}

/**
 * `base`, else "base 2", "base 3", … — the first whose name key isn't in
 * `taken`, each cut to fit the name limit. Mirrors the backend's rule.
 */
export function numberedName(base: string, taken: ReadonlySet<string>): string {
  const first = fitName(base, '');
  if (!taken.has(tagNameKey(first))) return first;
  for (let n = 2; ; n++) {
    const candidate = fitName(base, ` ${n}`);
    if (!taken.has(tagNameKey(candidate))) return candidate;
  }
}

export function indexTagCatalog(state: TagState): TagCatalog {
  const groups = [...state.groups].sort(byPosition);
  const groupsById = new Map(groups.map((group) => [group.id, group]));
  const tagsById = new Map<string, Tag>();
  const tagsByGroup = new Map<string | null, Tag[]>([[null, []]]);
  for (const group of groups) tagsByGroup.set(group.id, []);
  for (const tag of [...state.tags].sort(byPosition)) {
    // The backend's FK rules out a tag whose group is missing; should a local
    // patch ever leave one, it reads as unknown (and so do its assignments)
    // until the next snapshot.
    const scope = tagsByGroup.get(tag.groupId);
    if (!scope) continue;
    tagsById.set(tag.id, tag);
    scope.push(tag);
  }
  return {
    groups,
    groupsById,
    tagsById,
    tagsByGroup,
    iconGroup: groups.find((group) => group.isIconGroup) ?? null,
    sidebarGroups: groups.filter((group) => group.showInSidebar),
  };
}

/** The server's assignments by project, with assignments to unknown tags dropped. */
export function indexAssignments(
  state: TagState,
  tagsById: ReadonlyMap<string, Tag>
): Map<string, ReadonlySet<string>> {
  const byProject = new Map<string, Set<string>>();
  for (const { projectId, tagId } of state.assignments) {
    if (!tagsById.has(tagId)) continue;
    let ids = byProject.get(projectId);
    if (!ids) byProject.set(projectId, (ids = new Set()));
    ids.add(tagId);
  }
  return byProject;
}

// =============================================================================
// Snapshot reconciliation
// =============================================================================

/**
 * Whether two rows carry the same fields with the same values. Every field
 * of a group, a tag and an assignment is a primitive, so one level is the
 * whole comparison. A nested value would compare by identity and read as
 * changed, which costs a re-render, never a stale row.
 */
function sameRow<T extends object>(a: T, b: T): boolean {
  if (a === b) return true;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  // Both ways round, so a field only one of them carries counts.
  for (const key of Object.keys(left)) {
    if (!Object.is(left[key], right[key])) return false;
  }
  for (const key of Object.keys(right)) {
    if (!Object.is(left[key], right[key])) return false;
  }
  return true;
}

/**
 * `fetched`, with the row `current` holds under the same key in place of
 * each row that equals it: `current` itself when that leaves the same rows
 * in the same order.
 */
function reuseUnchangedRows<T extends object>(
  current: T[],
  fetched: readonly T[],
  keyOf: (row: T) => string
): T[] {
  const shown = new Map(current.map((row) => [keyOf(row), row]));
  let unchanged = current.length === fetched.length;
  const next = fetched.map((row, index) => {
    const existing = shown.get(keyOf(row));
    const kept = existing && sameRow(existing, row) ? existing : row;
    if (kept !== current[index]) unchanged = false;
    return kept;
  });
  return unchanged ? current : next;
}

function rowId(row: { id: string }): string {
  return row.id;
}

/** NUL can't occur in an id, so no two assignments share a key. */
function assignmentKey({ projectId, tagId }: ProjectTag): string {
  return `${projectId}\u0000${tagId}`;
}

/**
 * A fetched snapshot with every unchanged group, tag and assignment
 * replaced by the object `current` holds for it. A list that comes out with
 * the same objects in the same order is `current`'s list, and when all three
 * do the result is `current` itself. The result always equals `fetched`
 * value for value, in `fetched`'s order: only object identity differs.
 *
 * Every read of the backend builds new objects, so a snapshot applied as
 * fetched hands every reader a new tag, group and assignment for a change to
 * one, or to none. With the objects kept, a card whose tags did not change
 * is given the tags it already has, and an unchanged snapshot wakes nothing.
 */
export function reconcileTagState(current: TagState, fetched: TagState): TagState {
  // Spelled out, not spread, so a field added to TagState fails to compile
  // here until it is reconciled too.
  const next: TagState = {
    groups: reuseUnchangedRows(current.groups, fetched.groups, rowId),
    tags: reuseUnchangedRows(current.tags, fetched.tags, rowId),
    assignments: reuseUnchangedRows(current.assignments, fetched.assignments, assignmentKey),
  };
  const unchanged =
    next.groups === current.groups &&
    next.tags === current.tags &&
    next.assignments === current.assignments;
  return unchanged ? current : next;
}

// =============================================================================
// Pending writes
// =============================================================================

/**
 * What a write targets: `g:<groupId>` for a group's single choice,
 * `t:<tagId>` for one ungrouped tag. Writes to different slots commute in the
 * database; writes to one slot must reach it in click order.
 */
export type TagSlot = `g:${string}` | `t:${string}`;

export type TagIntent =
  | { kind: 'group'; groupId: string; tagId: string | null }
  | { kind: 'tag'; tagId: string; assigned: boolean };

export interface PendingTagIntent {
  intent: TagIntent;
  /** Unique per intent, so a settling write can tell whether it's still the latest. */
  token: number;
  /**
   * Null while the write is pending. Once it commits, the store's fetch seq at
   * that moment: only a snapshot requested after it (a higher seq) is sure to
   * include the write, so the intent stays until one lands.
   */
  confirmedAtSeq: number | null;
}

/** Per project, the latest intent in each slot. */
export type IntentOverlay = ReadonlyMap<string, ReadonlyMap<TagSlot, PendingTagIntent>>;

export function intentSlot(intent: TagIntent): TagSlot {
  return intent.kind === 'group' ? `g:${intent.groupId}` : `t:${intent.tagId}`;
}

/**
 * A project's effective tags: the server's set with each slot's latest intent
 * applied. A group intent drops that group's tags, then adds the chosen one.
 */
export function applyIntents(
  serverTagIds: ReadonlySet<string> | undefined,
  intents: ReadonlyMap<TagSlot, PendingTagIntent> | undefined,
  tagsById: ReadonlyMap<string, Tag>
): ReadonlySet<string> {
  const ids = new Set(serverTagIds);
  if (!intents) return ids;
  for (const { intent } of intents.values()) {
    if (intent.kind === 'group') {
      for (const id of ids) {
        if (tagsById.get(id)?.groupId === intent.groupId) ids.delete(id);
      }
      if (intent.tagId !== null && tagsById.get(intent.tagId)?.groupId === intent.groupId) {
        ids.add(intent.tagId);
      }
    } else if (intent.assigned) {
      if (tagsById.has(intent.tagId)) ids.add(intent.tagId);
    } else {
      ids.delete(intent.tagId);
    }
  }
  return ids;
}

/** Every project's effective tags. Projects without intents share the server's sets. */
export function effectiveProjectTagIds(
  server: ReadonlyMap<string, ReadonlySet<string>>,
  overlay: IntentOverlay,
  tagsById: ReadonlyMap<string, Tag>
): Map<string, ReadonlySet<string>> {
  const effective = new Map(server);
  for (const [projectId, intents] of overlay) {
    effective.set(projectId, applyIntents(server.get(projectId), intents, tagsById));
  }
  return effective;
}

export function currentIntent(
  overlay: IntentOverlay,
  projectId: string,
  slot: TagSlot
): PendingTagIntent | undefined {
  return overlay.get(projectId)?.get(slot);
}

/** Record `entry` as its slot's latest intent, replacing any older one. */
export function withIntent(
  overlay: IntentOverlay,
  projectId: string,
  entry: PendingTagIntent
): IntentOverlay {
  const intents = new Map(overlay.get(projectId));
  intents.set(intentSlot(entry.intent), entry);
  return new Map(overlay).set(projectId, intents);
}

/** Drop a slot's intent if it is still `token`'s; otherwise return `overlay` itself. */
export function withoutIntent(
  overlay: IntentOverlay,
  projectId: string,
  slot: TagSlot,
  token: number
): IntentOverlay {
  if (currentIntent(overlay, projectId, slot)?.token !== token) return overlay;
  const intents = new Map(overlay.get(projectId));
  intents.delete(slot);
  const next = new Map(overlay);
  if (intents.size === 0) next.delete(projectId);
  else next.set(projectId, intents);
  return next;
}

/** Mark a slot's intent committed at `seq`, if it is still `token`'s. */
export function withConfirmedIntent(
  overlay: IntentOverlay,
  projectId: string,
  slot: TagSlot,
  token: number,
  seq: number
): IntentOverlay {
  const entry = currentIntent(overlay, projectId, slot);
  if (entry?.token !== token) return overlay;
  return withIntent(overlay, projectId, { ...entry, confirmedAtSeq: seq });
}

/**
 * Drop the intents a snapshot requested at `snapshotSeq` already reflects:
 * those confirmed before it was requested. Pending ones stay. Returns
 * `overlay` itself when nothing changes.
 */
export function pruneConfirmedIntents(overlay: IntentOverlay, snapshotSeq: number): IntentOverlay {
  let next: Map<string, ReadonlyMap<TagSlot, PendingTagIntent>> | null = null;
  for (const [projectId, intents] of overlay) {
    const kept = [...intents].filter(
      ([, entry]) => entry.confirmedAtSeq === null || snapshotSeq <= entry.confirmedAtSeq
    );
    if (kept.length === intents.size) continue;
    next ??= new Map(overlay);
    if (kept.length === 0) next.delete(projectId);
    else next.set(projectId, new Map(kept));
  }
  return next ?? overlay;
}

// =============================================================================
// Lookups
// =============================================================================

/** A project's tag in one group, or null. */
export function groupTag(
  tagIds: ReadonlySet<string> | undefined,
  groupId: string,
  tagsById: ReadonlyMap<string, Tag>
): Tag | null {
  let found: Tag | null = null;
  for (const id of tagIds ?? []) {
    const tag = tagsById.get(id);
    // Single choice is enforced server-side; if two ever show up, the first
    // in position order wins so every surface agrees.
    if (tag?.groupId === groupId && (!found || tag.position < found.position)) found = tag;
  }
  return found;
}

/**
 * A project's tags as chips: grouped tags in group order, then ungrouped
 * tags in position order, skipping the groups named in `excludeGroupIds`
 * (the icon group, which the project icon already shows; a board's group).
 */
export function chipTags(
  tagIds: ReadonlySet<string> | undefined,
  catalog: TagCatalog,
  { excludeGroupIds = [] }: { excludeGroupIds?: readonly (string | null | undefined)[] } = {}
): Tag[] {
  if (!tagIds || tagIds.size === 0) return [];
  const excluded = new Set(excludeGroupIds.filter((id): id is string => !!id));
  const chips: Tag[] = [];
  for (const scope of [...catalog.groups.map((group) => group.id), null]) {
    if (scope !== null && excluded.has(scope)) continue;
    for (const tag of catalog.tagsByGroup.get(scope) ?? []) {
      if (tagIds.has(tag.id)) chips.push(tag);
    }
  }
  return chips;
}

/** The first `limit` chips, and how many more there are (a grid card's "+N"). */
export function limitChips(
  chips: readonly Tag[],
  limit: number
): { shown: Tag[]; overflow: number } {
  return { shown: chips.slice(0, limit), overflow: Math.max(0, chips.length - limit) };
}

/** How many of `projectIds` carry each tag. Tags nobody uses are absent. */
export function usageCounts(
  projectTagIds: ReadonlyMap<string, ReadonlySet<string>>,
  projectIds: Iterable<string>
): Map<string, number> {
  const counts = new Map<string, number>();
  for (const projectId of projectIds) {
    for (const tagId of projectTagIds.get(projectId) ?? []) {
      counts.set(tagId, (counts.get(tagId) ?? 0) + 1);
    }
  }
  return counts;
}
