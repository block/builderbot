/**
 * What a tag and a tag group dragged in Settings › Tags carry, and how each
 * drag is told from anything else dragged over the panel.
 *
 * Each carries a type of its own and nothing else. Some engines only start a
 * drag that carries data, and this is data enough. Neither carries a
 * `text/plain` copy (as for a board card or column, see boardDrop): a tag or
 * group released over a name field would paste its name into the field, and
 * the field's next blur would save that as a rename.
 */

/** The drag data type a tag row carries its tag's id under. */
export const TAG_DRAG_TYPE = 'application/x-staged-tag';

/** The drag data type a tag group carries its id under. */
export const TAG_GROUP_DRAG_TYPE = 'application/x-staged-tag-group';

/** Put a tag on a drag, under its own type only. */
export function writeTagDrag(dataTransfer: DataTransfer, tagId: string): void {
  dataTransfer.effectAllowed = 'move';
  dataTransfer.setData(TAG_DRAG_TYPE, tagId);
}

/** Put a group on a drag, under its own type only. */
export function writeTagGroupDrag(dataTransfer: DataTransfer, groupId: string): void {
  dataTransfer.effectAllowed = 'move';
  dataTransfer.setData(TAG_GROUP_DRAG_TYPE, groupId);
}

/**
 * Whether a drag event belongs to the drag of `type` the panel saw start.
 *
 * `started` alone can't say. It can be stale, from a drag whose source left
 * the page and whose dragend never came, and text or files dragged over the
 * panel after that are not it. So the types the drag carries have to agree:
 * `type`, or none at all, which is an engine that dropped the custom type
 * and left `started` the only word on it.
 *
 * A drag the panel did not see start is never one, whatever it carries: the
 * panel resolves a drop against the tag or group it saw picked up.
 */
function isStartedDrag(dataTransfer: DataTransfer | null, started: boolean, type: string): boolean {
  if (!started) return false;
  const types = dataTransfer?.types ?? [];
  return types.length === 0 || types.includes(type);
}

/** Whether a drag event belongs to the tag drag the panel saw start. */
export function isTagDrag(dataTransfer: DataTransfer | null, started: boolean): boolean {
  return isStartedDrag(dataTransfer, started, TAG_DRAG_TYPE);
}

/** Whether a drag event belongs to the group drag the panel saw start. */
export function isTagGroupDrag(dataTransfer: DataTransfer | null, started: boolean): boolean {
  return isStartedDrag(dataTransfer, started, TAG_GROUP_DRAG_TYPE);
}
