/**
 * What a tag group dragged in Settings › Tags carries, and how its drag is
 * told from anything else dragged over the panel.
 */

/** The drag data type a tag group carries its id under. */
export const TAG_GROUP_DRAG_TYPE = 'application/x-staged-tag-group';

/**
 * Put a group on a drag, under its own type only. Some engines only start a
 * drag that carries data, and this is data enough. No `text/plain` copy (as
 * for a board column, see boardDrop): a group released over a name field
 * would paste its name into the field, and the field's next blur would save
 * that as a rename.
 */
export function writeTagGroupDrag(dataTransfer: DataTransfer, groupId: string): void {
  dataTransfer.effectAllowed = 'move';
  dataTransfer.setData(TAG_GROUP_DRAG_TYPE, groupId);
}

/**
 * Whether a drag event belongs to the group drag the panel saw start.
 *
 * `started` alone can't say. It can be stale, from a drag whose source left
 * the page and whose dragend never came, and text or files dragged over the
 * panel after that are not the group. So the types the drag carries have to
 * agree: the group's own, or none at all, which is an engine that dropped
 * the custom type and left `started` the only word on it.
 *
 * A drag the panel did not see start is never one, whatever it carries: the
 * panel resolves a drop against the group it saw picked up.
 */
export function isTagGroupDrag(dataTransfer: DataTransfer | null, started: boolean): boolean {
  if (!started) return false;
  const types = dataTransfer?.types ?? [];
  return types.length === 0 || types.includes(TAG_GROUP_DRAG_TYPE);
}
