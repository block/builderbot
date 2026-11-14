/**
 * Reordering a board's columns: turning "beside this column" into a tag
 * move. A column is one of the group's tags, so the result is a tag drop
 * target, resolved and written like any other tag move (see tagReorder).
 *
 * A drop names the column under the pointer and a side, and is read against
 * the catalog and the hidden columns as they stand at drop time. Only tag
 * columns that are showing count. The tag lands next to the column the drop
 * named, so a hidden column is never the anchor and keeps its place among
 * the columns that did not move; a drop that leaves the showing columns in
 * the order they are in is no move at all, even where a hidden column sits
 * between the two. "No ‹group›" is not a tag and stays first: a drop on
 * either side of it puts the column first of those showing.
 */

import { NO_TAG_COLUMN_KEY } from './boardPrefs';
import type { TagDropTarget } from '../tags/tagReorder';
import type { TagCatalog } from '../tags/tagState';

/** Where a dragged column would land: beside another column, by its key. */
export interface ColumnDrop {
  /** A tag id, or NO_TAG_COLUMN_KEY. */
  columnKey: string;
  place: 'before' | 'after';
}

/** The group's tag columns that are showing, in order. None when the group is gone. */
function shownTagIds(
  catalog: Pick<TagCatalog, 'tagsByGroup'>,
  groupId: string,
  hidden: ReadonlySet<string>
): string[] {
  const tags = catalog.tagsByGroup.get(groupId) ?? [];
  return tags.filter((tag) => !hidden.has(tag.id)).map((tag) => tag.id);
}

/**
 * The tag drop target for dropping `tagId`'s column on `drop`, on `groupId`'s
 * board with the `hidden` columns left out. Null when nothing would change:
 * the group is gone, either column is no longer one of its showing tag
 * columns, the column is dropped on itself, or it would land where it
 * already is among the showing columns.
 */
export function columnDropTarget(
  catalog: Pick<TagCatalog, 'tagsByGroup'>,
  groupId: string,
  hidden: ReadonlySet<string>,
  tagId: string,
  drop: ColumnDrop
): TagDropTarget | null {
  const shown = shownTagIds(catalog, groupId, hidden);
  const from = shown.indexOf(tagId);
  if (from === -1) return null;
  const others = shown.filter((id) => id !== tagId);

  if (drop.columnKey === NO_TAG_COLUMN_KEY) {
    // First of the showing columns, on whichever side of "No ‹group›".
    if (from === 0) return null;
    return { groupId, anchorTagId: others[0], place: 'before' };
  }

  const anchor = others.indexOf(drop.columnKey);
  if (anchor === -1) return null;
  const index = drop.place === 'before' ? anchor : anchor + 1;
  if (index === from) return null;
  return { groupId, anchorTagId: drop.columnKey, place: drop.place };
}

/**
 * The tag drop target that moves `tagId`'s column one step left (`-1`) or
 * right (`1`): past the nearest showing tag column on that side, and with it
 * any hidden ones between. Null at that end, where "No ‹group›" or nothing
 * is next, and for a column that is not showing.
 */
export function columnStepTarget(
  catalog: Pick<TagCatalog, 'tagsByGroup'>,
  groupId: string,
  hidden: ReadonlySet<string>,
  tagId: string,
  step: -1 | 1
): TagDropTarget | null {
  const shown = shownTagIds(catalog, groupId, hidden);
  const from = shown.indexOf(tagId);
  const neighbour = from === -1 ? undefined : shown[from + step];
  if (neighbour === undefined) return null;
  return { groupId, anchorTagId: neighbour, place: step < 0 ? 'before' : 'after' };
}
