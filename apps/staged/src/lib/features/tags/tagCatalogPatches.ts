/**
 * Pure edits to a `get_tag_state` snapshot, mirroring what each confirmed
 * catalog write did in the backend (store/tags.rs), so the settings editor
 * shows the result before the refetch that tagsStore.patchLocal requests
 * lands. That refetch is the authority: a patch only has to be right enough
 * not to flash.
 */

import type { DeleteTagGroupMode, Tag, TagColor, TagGroup, TagState } from '../../types';
import { byPosition, numberedName, tagNameKey } from './tagState';

/** A tag move as `move_tag` takes it. */
export interface TagMove {
  tagId: string;
  /** The target scope: a group, or null for ungrouped. */
  groupId: string | null;
  /** Position in the target scope's tags without the moved tag; clamped. */
  index: number;
}

/** The scope's tags in order. */
function scopeTags(tags: readonly Tag[], groupId: string | null): Tag[] {
  return tags.filter((tag) => tag.groupId === groupId).sort(byPosition);
}

/** Give each tag in `order` its index as position, moving it into `groupId`. */
function placeTags(tags: readonly Tag[], order: readonly Tag[], groupId: string | null): Tag[] {
  const placed = new Map(order.map((tag, position) => [tag.id, position]));
  return tags.map((tag) => {
    const position = placed.get(tag.id);
    return position === undefined ? tag : { ...tag, groupId, position };
  });
}

/** Add a just-created tag, unless a refetch already has it. */
export function withTag(state: TagState, tag: Tag): TagState {
  if (state.tags.some((t) => t.id === tag.id)) return state;
  return { ...state, tags: [...state.tags, tag] };
}

/** Replace a tag's name, colour and icon. */
export function withTagValue(
  state: TagState,
  tagId: string,
  value: { name: string; color: TagColor; icon: string | null }
): TagState {
  const next = { ...value, name: value.name.trim() };
  return { ...state, tags: state.tags.map((t) => (t.id === tagId ? { ...t, ...next } : t)) };
}

/** Delete a tag and its assignments, renumbering its scope. */
export function withoutTag(state: TagState, tagId: string): TagState {
  const tag = state.tags.find((t) => t.id === tagId);
  if (!tag) return state;
  const tags = state.tags.filter((t) => t.id !== tagId);
  return {
    ...state,
    tags: placeTags(tags, scopeTags(tags, tag.groupId), tag.groupId),
    assignments: state.assignments.filter((a) => a.tagId !== tagId),
  };
}

/**
 * Move a tag within its scope or into another one. Moving into a group
 * drops it from projects that already have a tag there: the group's
 * existing value wins.
 */
export function withTagMoved(state: TagState, move: TagMove): TagState {
  const tag = state.tags.find((t) => t.id === move.tagId);
  if (!tag) return state;
  const order = scopeTags(state.tags, move.groupId).filter((t) => t.id !== tag.id);
  order.splice(Math.min(Math.max(0, move.index), order.length), 0, tag);
  let tags = placeTags(state.tags, order, move.groupId);
  if (tag.groupId === move.groupId) return { ...state, tags };

  tags = placeTags(tags, scopeTags(tags, tag.groupId), tag.groupId);
  let assignments = state.assignments;
  if (move.groupId !== null) {
    const inGroup = new Set(
      state.tags.filter((t) => t.groupId === move.groupId && t.id !== tag.id).map((t) => t.id)
    );
    const keepTheirs = new Set(
      assignments.filter((a) => inGroup.has(a.tagId)).map((a) => a.projectId)
    );
    assignments = assignments.filter((a) => a.tagId !== tag.id || !keepTheirs.has(a.projectId));
  }
  return { ...state, tags, assignments };
}

/** Add a just-created group, unless a refetch already has it. */
export function withGroup(state: TagState, group: TagGroup): TagState {
  if (state.groups.some((g) => g.id === group.id)) return state;
  return { ...state, groups: [...state.groups, group] };
}

export function withGroupName(state: TagState, groupId: string, name: string): TagState {
  return {
    ...state,
    groups: state.groups.map((g) => (g.id === groupId ? { ...g, name: name.trim() } : g)),
  };
}

/** Make `groupId` the icon group, or turn the icon group off with null. */
export function withIconGroup(state: TagState, groupId: string | null): TagState {
  return {
    ...state,
    groups: state.groups.map((g) =>
      g.isIconGroup === (g.id === groupId) ? g : { ...g, isIconGroup: g.id === groupId }
    ),
  };
}

/** Pin `groupId`'s board to the sidebar, or unpin it. */
export function withGroupSidebar(state: TagState, groupId: string, shown: boolean): TagState {
  return {
    ...state,
    groups: state.groups.map((g) =>
      g.id === groupId && g.showInSidebar !== shown ? { ...g, showInSidebar: shown } : g
    ),
  };
}

/** Number the groups in `groupIds` order. Groups not listed keep their position. */
export function withGroupOrder(state: TagState, groupIds: readonly string[]): TagState {
  const placed = new Map(groupIds.map((id, position) => [id, position]));
  return {
    ...state,
    groups: state.groups.map((g) => {
      const position = placed.get(g.id);
      return position === undefined ? g : { ...g, position };
    }),
  };
}

/**
 * Delete a group. `ungroup` appends its tags after the ungrouped ones,
 * keeping their assignments; a name an ungrouped tag already has becomes
 * "Name (Group)", then "Name (Group) 2", …. `deleteTags` deletes the tags
 * and their assignments.
 */
export function withoutGroup(state: TagState, groupId: string, mode: DeleteTagGroupMode): TagState {
  const group = state.groups.find((g) => g.id === groupId);
  if (!group) return state;
  const groups = [...state.groups]
    .filter((g) => g.id !== groupId)
    .sort(byPosition)
    .map((g, position) => (g.position === position ? g : { ...g, position }));
  const moving = scopeTags(state.tags, groupId);

  if (mode === 'deleteTags') {
    const deleted = new Set(moving.map((t) => t.id));
    return {
      groups,
      tags: state.tags.filter((t) => !deleted.has(t.id)),
      assignments: state.assignments.filter((a) => !deleted.has(a.tagId)),
    };
  }

  const ungrouped = scopeTags(state.tags, null);
  const taken = new Set(ungrouped.map((t) => tagNameKey(t.name)));
  const renamed = new Map<string, string>();
  for (const tag of moving) {
    const name = taken.has(tagNameKey(tag.name))
      ? numberedName(`${tag.name} (${group.name})`, taken)
      : tag.name;
    taken.add(tagNameKey(name));
    renamed.set(tag.id, name);
  }
  const tags = placeTags(state.tags, [...ungrouped, ...moving], null).map((t) => {
    const name = renamed.get(t.id);
    return name === undefined || name === t.name ? t : { ...t, name };
  });
  return { ...state, groups, tags };
}
