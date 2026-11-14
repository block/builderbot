/**
 * Dragging on the board: a card between columns, or a column among the
 * others. What a card's drag carries and what its drop means, and where a
 * dragged column's pointer puts it.
 *
 * The two are different kinds of drag. A card's is an HTML5 drag. A column's
 * is made of pointer events (see shared/pointerDrag), so it carries no data
 * and the board resolves each point the pointer reaches from the columns'
 * rects, forgiving a pointer that strays a little off the row.
 *
 * A card's drop names the column, not a tag fetched when the drag began, and
 * is resolved against the catalog as it stands at drop time: a column whose
 * tag was deleted or moved to another group mid-drag resolves to nothing
 * rather than to a stale write. A column's drop names the column under the
 * pointer and a side of it (see boardColumnReorder for what that moves).
 */

import {
  dragPositionAlong,
  spanAlong,
  type DragBox,
  type PointerDragPoint,
} from '../../shared/pointerDrag';
import type { ColumnDrop } from './boardColumnReorder';
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

/**
 * Whether a drag event is a board card's. The type the drag carries decides.
 * `started`, whether the board saw a card drag start, speaks only for a drag
 * that carries no types at all, which is an engine that dropped them. It can
 * be stale, from a drag whose card left the page and whose dragend never
 * came, and text or files dragged over the board after that are not a card.
 */
export function isCardDrag(dataTransfer: DataTransfer | null, started: boolean): boolean {
  const types = dataTransfer?.types ?? [];
  if (types.includes(PROJECT_DRAG_TYPE)) return true;
  return types.length === 0 && started;
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

/** A column in the row, and where it is on screen. */
export interface ColumnBox {
  key: string;
  rect: Pick<DOMRect, 'left' | 'right' | 'width'>;
}

/**
 * Where a dragged column would land for a pointer at `point`, given the row
 * of columns, the columns in it in order, and `last`, where the pointer last
 * put it.
 *
 * Only how far across the row the pointer is decides. A column dragged by
 * its header drifts above the row, and one dragged past a short column is
 * under it: either way it lands beside the column the pointer is level
 * with, on the pointer's side. Past the last column (over the + column,
 * say) it lands after the last, and before the first, before it.
 *
 * The gap between two columns is where the drop line for that place is
 * drawn, so a pointer in it lands the column there. That place has two
 * names, after the one column and before the other, which differ only when
 * a hidden column sits between the two. `last` is kept when it is either,
 * and the place is after the first column otherwise.
 *
 * The row scrolls, so a column can run past its edge. A pointer past either
 * end of the row is taken to be at that end, and lands beside what shows
 * there, which the row is scrolling on from.
 *
 * All of that holds within `DRAG_REACH` of the row, where there is always
 * somewhere to land. Further out the pointer is clearly away and the column
 * lands nowhere.
 */
export function columnAtPoint(
  point: PointerDragPoint,
  row: DragBox,
  columns: readonly ColumnBox[],
  last: ColumnDrop | null
): ColumnDrop | null {
  const x = dragPositionAlong(point, row, 'x');
  if (x === null) return null;
  const hit = spanAlong(
    x,
    columns.map(({ rect }) => ({ start: rect.left, end: rect.right }))
  );
  if (!hit) return null;
  const over = columns[hit.index];
  if (!hit.gap) return { columnKey: over.key, place: columnDropSide(over.key, over.rect, x) };
  const next = columns[hit.index + 1];
  const named =
    last?.place === 'after' ? last.columnKey === over.key : last?.columnKey === next.key;
  return named && last ? last : { columnKey: over.key, place: 'after' };
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
