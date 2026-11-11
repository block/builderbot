/**
 * Dragging a card between board columns: what a drag carries, and what a
 * drop means.
 *
 * A drop names the column, not a tag fetched when the drag began, and is
 * resolved against the catalog as it stands at drop time: a column whose tag
 * was deleted or moved to another group mid-drag resolves to nothing rather
 * than to a stale write.
 */

import { NO_TAG_COLUMN_KEY } from './boardPrefs';
import type { TagCatalog } from '../tags/tagState';

/** The drag data type a board card carries its project id under. */
export const PROJECT_DRAG_TYPE = 'application/x-staged-project';

/**
 * Put a card's project on a drag, under its own type only. No `text/plain`
 * copy: a card released over the search field, or any text input, would
 * paste the project's id into it. The board's own fallback for engines that
 * drop custom types is the card it saw the drag start on (see
 * `readProjectDrag`), so the copy isn't needed to move a card.
 */
export function writeProjectDrag(dataTransfer: DataTransfer, projectId: string): void {
  dataTransfer.effectAllowed = 'move';
  dataTransfer.setData(PROJECT_DRAG_TYPE, projectId);
}

/**
 * The project a drop carries: the drag data, else `fallback` (the card the
 * board saw the drag start on), as long as it names one of `projectIds`.
 * Text dragged in from elsewhere names none, so it never moves a card.
 */
export function readProjectDrag(
  dataTransfer: DataTransfer | null,
  fallback: string | null,
  projectIds: ReadonlySet<string>
): string | null {
  const candidates = [dataTransfer?.getData(PROJECT_DRAG_TYPE), fallback];
  return candidates.find((id): id is string => !!id && projectIds.has(id)) ?? null;
}

/** The write a drop asks for: the project's tag in the group, or null to clear it. */
export interface BoardDropWrite {
  groupId: string;
  tagId: string | null;
}

/**
 * Resolve a drop on `columnKey` of `groupId`'s board, or null when the group
 * is gone or the column's tag is no longer in it.
 */
export function resolveBoardDrop(
  columnKey: string,
  groupId: string,
  catalog: Pick<TagCatalog, 'groupsById' | 'tagsById'>
): BoardDropWrite | null {
  if (!catalog.groupsById.has(groupId)) return null;
  if (columnKey === NO_TAG_COLUMN_KEY) return { groupId, tagId: null };
  const tag = catalog.tagsById.get(columnKey);
  return tag?.groupId === groupId ? { groupId, tagId: tag.id } : null;
}
