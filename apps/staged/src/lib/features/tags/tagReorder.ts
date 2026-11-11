/**
 * Pure helpers for moving tags in Settings › Tags: drag-and-drop, and the
 * row menu's Move up / Move down / Move to group.
 *
 * A drag spans many renders, and the catalog can change under it (a
 * `tags-changed` refetch from another window), so a drop target names a
 * neighbouring tag rather than an index and is resolved against the catalog
 * as it is at drop time (the repoReorder pattern).
 */

import type { Tag, TagGroup } from '../../types';
import type { TagMove } from './tagCatalogPatches';
import { tagNameKey, type TagCatalog } from './tagState';

/** Where a dragged tag would land: beside another tag, or at the end of a scope. */
export type TagDropTarget =
  | { groupId: string | null; anchorTagId: string; place: 'before' | 'after' }
  | { groupId: string | null; anchorTagId: null };

export interface ResolvedTagMove extends TagMove {
  /** Whether the tag changes scope, rather than reordering within its own. */
  crossScope: boolean;
  /**
   * How many of the given projects carry the tag and already have another
   * tag in the target group. Moving keeps theirs and drops this one. Zero
   * unless the tag moves into a group from elsewhere.
   */
  conflicts: number;
  /**
   * A tag in the target scope with the same name. The backend would refuse
   * the move, so the user renames one first.
   */
  collision: Tag | null;
}

/**
 * Resolve dropping `tagId` on `target`. Null when nothing would change: the
 * tag or target group is gone, the anchor is the tag itself or left the
 * target scope mid-drag, or the tag would land where it already is.
 *
 * `projectTagIds` is each project's tags, for the conflict count.
 */
export function resolveTagDrop(
  catalog: TagCatalog,
  tagId: string,
  target: TagDropTarget,
  projectTagIds: Iterable<ReadonlySet<string>> = []
): ResolvedTagMove | null {
  const tag = catalog.tagsById.get(tagId);
  const scope = catalog.tagsByGroup.get(target.groupId);
  if (!tag || !scope) return null;
  if (target.anchorTagId === tagId) return null;

  const others = scope.filter((t) => t.id !== tagId);
  let index = others.length;
  if (target.anchorTagId !== null) {
    const anchor = others.findIndex((t) => t.id === target.anchorTagId);
    if (anchor === -1) return null;
    index = target.place === 'before' ? anchor : anchor + 1;
  }

  const crossScope = tag.groupId !== target.groupId;
  if (!crossScope && scope.indexOf(tag) === index) return null;

  let conflicts = 0;
  if (crossScope && target.groupId !== null) {
    const siblings = new Set(others.map((t) => t.id));
    for (const ids of projectTagIds) {
      if (!ids.has(tagId)) continue;
      for (const id of ids) {
        if (siblings.has(id)) {
          conflicts++;
          break;
        }
      }
    }
  }

  const key = tagNameKey(tag.name);
  const collision = crossScope ? (others.find((t) => tagNameKey(t.name) === key) ?? null) : null;

  return { tagId, groupId: target.groupId, index, crossScope, conflicts, collision };
}

/**
 * The target that moves a tag one step up (`-1`) or down (`1`) within its
 * scope, or null at that end.
 */
export function stepTarget(catalog: TagCatalog, tagId: string, step: -1 | 1): TagDropTarget | null {
  const tag = catalog.tagsById.get(tagId);
  const scope = tag ? catalog.tagsByGroup.get(tag.groupId) : undefined;
  if (!tag || !scope) return null;
  const neighbour = scope[scope.indexOf(tag) + step];
  if (!neighbour) return null;
  return {
    groupId: tag.groupId,
    anchorTagId: neighbour.id,
    place: step < 0 ? 'before' : 'after',
  };
}

/** Group ids with `groupId` moved one step up (`-1`) or down (`1`), or null at that end. */
export function steppedGroupOrder(
  groups: readonly TagGroup[],
  groupId: string,
  step: -1 | 1
): string[] | null {
  const ids = groups.map((group) => group.id);
  const from = ids.indexOf(groupId);
  const to = from + step;
  if (from === -1 || to < 0 || to >= ids.length) return null;
  [ids[from], ids[to]] = [ids[to], ids[from]];
  return ids;
}
