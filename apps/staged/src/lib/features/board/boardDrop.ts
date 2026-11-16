/**
 * Dragging on the board: a card between columns, or a column among the
 * others. What a card's drop means, and where a dragged column's pointer
 * puts it.
 *
 * Both are made of pointer events (boardCardDrag and shared/pointerDrag), so
 * neither carries data: the board resolves what the pointer is over itself,
 * a card's from the element under it and a column's from the columns' rects,
 * forgiving a pointer that strays a little off the row.
 *
 * A card's drop names the column, not a tag fetched when the drag began, and
 * is judged against the catalog as it stands at drop time: a column whose
 * tag was deleted or moved to another group mid-drag is refused rather than
 * turned into a stale write. A column's drop names the column under the
 * pointer and a side of it (see boardColumnReorder for what that moves).
 */

import {
  dragPositionAlong,
  spanAlong,
  type DragBox,
  type PointerDragPoint,
} from '../../shared/pointerDrag';
import type { BoardDropTarget } from './boardCardDrag';
import type { ColumnDrop } from './boardColumnReorder';
import { NO_TAG_COLUMN_KEY } from './boardPrefs';
import type { TagCatalog } from '../tags/tagState';

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
 * What releasing the card would do:
 * - `none`: it is over nothing, and goes back where it was.
 * - `home`: it is over its own column, so there is nothing to change.
 * - `refused`: it is over something that can't take it, and goes back.
 * - `move`: it changes column, by `write`.
 */
export type BoardDropVerdict =
  | { kind: 'none' }
  | { kind: 'home' }
  | { kind: 'refused' }
  | { kind: 'move'; key: string; write: BoardDropWrite };

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

/**
 * Judge a card over `target` on `groupId`'s board. `homeKey` is the column
 * the card is in now; "No ‹group›" clears the group's tag, as it does in the
 * card's "Move to" menu. The + column takes no cards, and neither does a
 * column the catalog no longer backs.
 */
export function judgeBoardDrop(
  target: BoardDropTarget | null,
  homeKey: string | null,
  groupId: string,
  catalog: Pick<TagCatalog, 'groupsById' | 'tagsById'>
): BoardDropVerdict {
  if (!target) return { kind: 'none' };
  if (target.kind === 'add-column') return { kind: 'refused' };
  if (target.key === homeKey) return { kind: 'home' };
  const write = resolveBoardDrop(target.key, groupId, catalog);
  return write ? { kind: 'move', key: target.key, write } : { kind: 'refused' };
}
