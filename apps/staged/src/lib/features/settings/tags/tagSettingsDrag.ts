/**
 * Dragging in Settings › Tags: a tag among the rows, or a group among the
 * others. What a tag's drag carries and how it is told from anything else
 * dragged over the panel, and where a dragged group's pointer puts it.
 *
 * The two are different kinds of drag. A tag's is an HTML5 drag. A group's
 * is made of pointer events (see shared/pointerDrag), so it carries no data
 * and the panel resolves each point the pointer reaches from the sections'
 * rects.
 *
 * A tag carries a type of its own and nothing else. Some engines only start
 * a drag that carries data, and this is data enough. It carries no
 * `text/plain` copy (as for a board card, see boardDrop): a tag released
 * over a name field would paste its name into the field, and the field's
 * next blur would save that as a rename.
 */

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

/** The edges of a box in client coordinates, as `getBoundingClientRect` gives them. */
type Box = Pick<DOMRect, 'left' | 'right' | 'top' | 'bottom'>;

function boxHolds(box: Box, point: { clientX: number; clientY: number }): boolean {
  return (
    point.clientX >= box.left &&
    point.clientX < box.right &&
    point.clientY >= box.top &&
    point.clientY < box.bottom
  );
}

/** A group's section in the list, and where it is on screen. */
export interface GroupSectionBox {
  groupId: string;
  rect: Box & Pick<DOMRect, 'height'>;
}

/**
 * Where a dragged group would land for a pointer at `point`, given the list
 * of sections, the sections in it, and `last`, where the pointer last put
 * it.
 *
 * Over a section, it lands beside that group: above it on the section's
 * upper half, below it on the lower. In the list but over no section, it
 * stays where it last was: the drop line is drawn in the gap between two
 * sections, which is neither of them, and the same goes for the ungrouped
 * tags and the space under them. Outside the list it lands nowhere. The
 * panel keeps each answer as the next `last`, so a pointer that comes back
 * over a gap has nowhere to land until it crosses a section.
 *
 * The list scrolls, so a section can run past its edge: the list is asked
 * first, and a section counts only where it shows.
 */
export function groupAtPoint(
  point: { clientX: number; clientY: number },
  list: Box,
  sections: readonly GroupSectionBox[],
  last: GroupDropTarget | null
): GroupDropTarget | null {
  if (!boxHolds(list, point)) return null;
  const over = sections.find((section) => boxHolds(section.rect, point));
  if (!over) return last;
  return {
    anchorGroupId: over.groupId,
    place: point.clientY < over.rect.top + over.rect.height / 2 ? 'before' : 'after',
  };
}
