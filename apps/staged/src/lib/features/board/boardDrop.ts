/**
 * Dragging on the board: a card between columns, or a column among the
 * others. What each drag carries, how the two are told apart, and what a
 * drop means.
 *
 * A card's drop names the column, not a tag fetched when the drag began, and
 * is resolved against the catalog as it stands at drop time: a column whose
 * tag was deleted or moved to another group mid-drag resolves to nothing
 * rather than to a stale write. A column's drop names the column under the
 * pointer and a side of it (see boardColumnReorder for what that moves).
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

/** The drag data type a board column carries its key (its tag's id) under. */
export const COLUMN_DRAG_TYPE = 'application/x-staged-board-column';

/**
 * Put a column on a drag, under its own type only: as for a card, no
 * `text/plain` copy to paste into a text input, and a type of its own so a
 * column never lands as a card.
 */
export function writeColumnDrag(dataTransfer: DataTransfer, columnKey: string): void {
  dataTransfer.effectAllowed = 'move';
  dataTransfer.setData(COLUMN_DRAG_TYPE, columnKey);
}

/**
 * The column a drop carries: the drag data, else `fallback` (the column the
 * board saw the drag start on). Not checked against the board's columns
 * here: `columnDropTarget` moves only a tag column that is showing.
 */
export function readColumnDrag(
  dataTransfer: DataTransfer | null,
  fallback: string | null
): string | null {
  return dataTransfer?.getData(COLUMN_DRAG_TYPE) || fallback || null;
}

/** The two things that drag on a board. */
export type BoardDragKind = 'card' | 'column';

/**
 * Which kind of drag an event belongs to, or null for one that is not the
 * board's (files, text). The types the drag carries decide. `started`, the
 * kind the board saw start, speaks only for an engine that dropped them: it
 * can be stale, from a drag whose source left the page and whose dragend
 * never came.
 */
export function boardDragKind(
  dataTransfer: DataTransfer | null,
  started: BoardDragKind | null
): BoardDragKind | null {
  const types = dataTransfer?.types ?? [];
  if (types.includes(COLUMN_DRAG_TYPE)) return 'column';
  if (types.includes(PROJECT_DRAG_TYPE)) return 'card';
  return started;
}

/**
 * The side of a column a dragged column would land on, for a pointer at
 * `clientX` over it: before it on its left half, after it on its right.
 * "No ‹group›" stays first, so a column only ever lands after it.
 */
export function columnDropSide(
  columnKey: string,
  rect: Pick<DOMRect, 'left' | 'width'>,
  clientX: number
): 'before' | 'after' {
  if (columnKey === NO_TAG_COLUMN_KEY) return 'after';
  return clientX < rect.left + rect.width / 2 ? 'before' : 'after';
}

/**
 * Whether a drag that fired a dragleave is still over `container`: what it
 * moved on to (an element, when the event names one) is inside. Not every
 * engine fills in a dragleave's `relatedTarget`, so without one the pointer
 * decides.
 */
export function dragStillInside(
  container: Pick<Element, 'contains' | 'getBoundingClientRect'>,
  event: Pick<DragEvent, 'relatedTarget' | 'clientX' | 'clientY'>
): boolean {
  const next = event.relatedTarget as Node | null;
  if (next) return container.contains(next);
  const box = container.getBoundingClientRect();
  return (
    event.clientX >= box.left &&
    event.clientX < box.right &&
    event.clientY >= box.top &&
    event.clientY < box.bottom
  );
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
