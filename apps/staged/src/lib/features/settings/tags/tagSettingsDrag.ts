/**
 * Dragging in Settings › Tags: a tag among the rows, or a group among the
 * others. What a tag's drag carries and how it is told from anything else
 * dragged over the panel, and where a dragged group's pointer puts it.
 *
 * The two are different kinds of drag. A tag's is an HTML5 drag. A group's
 * is made of pointer events (see shared/pointerDrag), so it carries no data
 * and the panel resolves each point the pointer reaches from the sections'
 * rects, forgiving a pointer that strays a little off the list.
 *
 * A tag carries a type of its own and nothing else. Some engines only start
 * a drag that carries data, and this is data enough. It carries no
 * `text/plain` copy (as for a board card, see boardDrop): a tag released
 * over a name field would paste its name into the field, and the field's
 * next blur would save that as a rename.
 */

import {
  dragPositionAlong,
  spanAlong,
  type DragBox,
  type PointerDragPoint,
} from '../../../shared/pointerDrag';
import type { GroupDropTarget } from '../../tags/tagReorder';

/** The drag data type a tag row carries its tag's id under. */
export const TAG_DRAG_TYPE = 'application/x-staged-tag';

/** Put a tag on a drag, under its own type only. */
export function writeTagDrag(dataTransfer: DataTransfer, tagId: string): void {
  dataTransfer.effectAllowed = 'move';
  dataTransfer.setData(TAG_DRAG_TYPE, tagId);
}

/**
 * Whether a drag event belongs to the tag drag the panel saw start.
 *
 * `started` alone can't say. It can be stale, from a drag whose row left the
 * page and whose dragend never came, and text or files dragged over the
 * panel after that are not it. So the types the drag carries have to agree:
 * the tag type, or none at all, which is an engine that dropped the custom
 * type and left `started` the only word on it.
 *
 * A drag the panel did not see start is never one, whatever it carries: the
 * panel resolves a drop against the tag it saw picked up.
 */
export function isTagDrag(dataTransfer: DataTransfer | null, started: boolean): boolean {
  if (!started) return false;
  const types = dataTransfer?.types ?? [];
  return types.length === 0 || types.includes(TAG_DRAG_TYPE);
}

/** A group's section in the list, and where it is on screen. */
export interface GroupSectionBox {
  groupId: string;
  rect: Pick<DOMRect, 'top' | 'bottom' | 'height'>;
}

/**
 * Where a dragged group would land for a pointer at `point`, given the list
 * of sections, the sections in it in order, and `last`, where the pointer
 * last put it.
 *
 * Only how far down the list the pointer is decides. A group dragged by its
 * handle, which is at the list's left edge, drifts off that edge, and the
 * sections stop short of a wide list's right: either way the group lands
 * beside the section the pointer is level with, above it on the section's
 * upper half and below it on the lower. Below the last section (over the
 * ungrouped tags, say) it lands after the last group, and above the first,
 * before it.
 *
 * The gap between two sections is where the drop line for that place is
 * drawn, so a pointer in it lands the group there. That place has two
 * names, below the one group and above the other. `last` is kept when it is
 * either, so the line does not change hands, and the place is below the
 * first group otherwise.
 *
 * The list scrolls, so a section can run past its edge. A pointer past
 * either end of the list is taken to be at that end, and the group lands
 * beside what shows there, which the list is scrolling on from.
 *
 * All of that holds within `DRAG_REACH` of the list, where there is always
 * somewhere to land. Further out the pointer is clearly away and the group
 * lands nowhere.
 */
export function groupAtPoint(
  point: PointerDragPoint,
  list: DragBox,
  sections: readonly GroupSectionBox[],
  last: GroupDropTarget | null
): GroupDropTarget | null {
  const y = dragPositionAlong(point, list, 'y');
  if (y === null) return null;
  const hit = spanAlong(
    y,
    sections.map(({ rect }) => ({ start: rect.top, end: rect.bottom }))
  );
  if (!hit) return null;
  const over = sections[hit.index];
  if (!hit.gap) {
    return {
      anchorGroupId: over.groupId,
      place: y < over.rect.top + over.rect.height / 2 ? 'before' : 'after',
    };
  }
  const next = sections[hit.index + 1];
  const named =
    last?.place === 'after'
      ? last.anchorGroupId === over.groupId
      : last?.anchorGroupId === next.groupId;
  return named && last ? last : { anchorGroupId: over.groupId, place: 'after' };
}
