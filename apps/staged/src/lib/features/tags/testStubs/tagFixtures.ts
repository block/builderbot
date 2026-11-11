/** Tag and group fixtures shared by the tag tests. */

import type { Tag, TagGroup, TagState } from '../../../types';

export function group(id: string, position: number, overrides: Partial<TagGroup> = {}): TagGroup {
  return {
    id,
    name: id,
    position,
    isIconGroup: false,
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
