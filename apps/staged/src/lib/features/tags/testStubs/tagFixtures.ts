/** Tag and group fixtures shared by the tag tests. */

import type { Tag, TagGroup, TagState } from '../../../types';

export function group(id: string, position: number, overrides: Partial<TagGroup> = {}): TagGroup {
  return {
    id,
    name: id,
    position,
    isIconGroup: false,
    showInSidebar: false,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

export function tag(
  id: string,
  groupId: string | null,
  position: number,
  overrides: Partial<Tag> = {}
): Tag {
  return {
    id,
    groupId,
    name: id,
    color: 'gray',
    icon: null,
    position,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

/**
 * Status (the icon group: idea, doing, done) and Priority (p0, p1), plus
 * ungrouped q4 and infra.
 */
export function tagState(assignments: TagState['assignments'] = []): TagState {
  return {
    groups: [
      group('status', 0, { name: 'Status', isIconGroup: true }),
      group('priority', 1, { name: 'Priority' }),
    ],
    tags: [
      tag('idea', 'status', 0, { name: 'Idea' }),
      tag('doing', 'status', 1, { name: 'Doing' }),
      tag('done', 'status', 2, { name: 'Done' }),
      tag('p0', 'priority', 0, { name: 'P0' }),
      tag('p1', 'priority', 1, { name: 'P1' }),
      tag('q4', null, 0, { name: 'q4' }),
      tag('infra', null, 1, { name: 'infra' }),
    ],
    assignments,
  };
}

/** A scope's tag ids in position order. */
export function scopeIds(state: TagState, groupId: string | null): string[] {
  return state.tags
    .filter((t) => t.groupId === groupId)
    .sort((a, b) => a.position - b.position)
    .map((t) => t.id);
}

/** The state after `set_project_group_tag`, as the backend applies it. */
export function withGroupTag(
  state: TagState,
  projectId: string,
  groupId: string,
  tagId: string | null
): TagState {
  const inGroup = new Set(state.tags.filter((t) => t.groupId === groupId).map((t) => t.id));
  return {
    ...state,
    assignments: [
      ...state.assignments.filter((a) => a.projectId !== projectId || !inGroup.has(a.tagId)),
      ...(tagId ? [{ projectId, tagId }] : []),
    ],
  };
}

/** The state after `set_project_tag_assigned` for an ungrouped tag. */
export function withTagAssigned(
  state: TagState,
  projectId: string,
  tagId: string,
  assigned: boolean
): TagState {
  const rest = state.assignments.filter((a) => a.projectId !== projectId || a.tagId !== tagId);
  return { ...state, assignments: assigned ? [...rest, { projectId, tagId }] : rest };
}
