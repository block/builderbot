import { describe, expect, it } from 'vitest';
import {
  withGroup,
  withGroupName,
  withGroupOrder,
  withGroupSidebar,
  withIconGroup,
  withTag,
  withTagMoved,
  withTagValue,
  withoutGroup,
  withoutTag,
} from './tagCatalogPatches';
import { group, scopeIds, tag, tagState } from './testStubs/tagFixtures';

function positions(state: ReturnType<typeof tagState>, groupId: string | null): number[] {
  return state.tags
    .filter((t) => t.groupId === groupId)
    .map((t) => t.position)
    .sort((a, b) => a - b);
}

describe('withTag / withGroup', () => {
  it('appends a created tag once', () => {
    const created = tag('new', 'status', 3);
    const once = withTag(tagState(), created);
    expect(scopeIds(once, 'status')).toEqual(['idea', 'doing', 'done', 'new']);
    expect(withTag(once, created)).toBe(once);
  });

  it('appends a created group once', () => {
    const created = group('area', 2);
    const once = withGroup(tagState(), created);
    expect(once.groups.map((g) => g.id)).toEqual(['status', 'priority', 'area']);
    expect(withGroup(once, created)).toBe(once);
  });
});

describe('withTagValue', () => {
  it('replaces name (trimmed), colour and icon', () => {
    const next = withTagValue(tagState(), 'idea', { name: ' Spark ', color: 'red', icon: 'flame' });
    expect(next.tags.find((t) => t.id === 'idea')).toMatchObject({
      name: 'Spark',
      color: 'red',
      icon: 'flame',
    });
  });
});

describe('withoutTag', () => {
  it('drops the tag and its assignments, and renumbers its scope', () => {
    const next = withoutTag(
      tagState([
        { projectId: 'a', tagId: 'doing' },
        { projectId: 'a', tagId: 'q4' },
      ]),
      'doing'
    );
    expect(scopeIds(next, 'status')).toEqual(['idea', 'done']);
    expect(positions(next, 'status')).toEqual([0, 1]);
    expect(next.assignments).toEqual([{ projectId: 'a', tagId: 'q4' }]);
  });
});

describe('withTagMoved', () => {
  it('reorders within a scope', () => {
    const next = withTagMoved(tagState(), { tagId: 'idea', groupId: 'status', index: 2 });
    expect(scopeIds(next, 'status')).toEqual(['doing', 'done', 'idea']);
    expect(positions(next, 'status')).toEqual([0, 1, 2]);
  });

  it('clamps the index', () => {
    const next = withTagMoved(tagState(), { tagId: 'done', groupId: 'status', index: -4 });
    expect(scopeIds(next, 'status')).toEqual(['done', 'idea', 'doing']);
  });

  it('moves into a group, renumbering both scopes', () => {
    const next = withTagMoved(tagState(), { tagId: 'q4', groupId: 'status', index: 1 });
    expect(scopeIds(next, 'status')).toEqual(['idea', 'q4', 'doing', 'done']);
    expect(positions(next, 'status')).toEqual([0, 1, 2, 3]);
    expect(scopeIds(next, null)).toEqual(['infra']);
    expect(positions(next, null)).toEqual([0]);
  });

  it('keeps a project’s existing tag in the target group, dropping the moved one', () => {
    const next = withTagMoved(
      tagState([
        { projectId: 'a', tagId: 'q4' },
        { projectId: 'a', tagId: 'doing' },
        { projectId: 'b', tagId: 'q4' },
      ]),
      { tagId: 'q4', groupId: 'status', index: 0 }
    );
    expect(next.assignments).toEqual([
      { projectId: 'a', tagId: 'doing' },
      { projectId: 'b', tagId: 'q4' },
    ]);
  });

  it('keeps every assignment moving out to the ungrouped tags', () => {
    const assignments = [
      { projectId: 'a', tagId: 'p0' },
      { projectId: 'a', tagId: 'infra' },
    ];
    const next = withTagMoved(tagState(assignments), { tagId: 'p0', groupId: null, index: 9 });
    expect(scopeIds(next, null)).toEqual(['q4', 'infra', 'p0']);
    expect(scopeIds(next, 'priority')).toEqual(['p1']);
    expect(next.assignments).toEqual(assignments);
  });
});

describe('group patches', () => {
  it('renames a group, trimmed', () => {
    const next = withGroupName(tagState(), 'priority', ' Urgency ');
    expect(next.groups.find((g) => g.id === 'priority')?.name).toBe('Urgency');
  });

  it('moves the icon flag, or clears it', () => {
    const moved = withIconGroup(tagState(), 'priority');
    expect(moved.groups.filter((g) => g.isIconGroup).map((g) => g.id)).toEqual(['priority']);
    const off = withIconGroup(moved, null);
    expect(off.groups.some((g) => g.isIconGroup)).toBe(false);
  });

  it("pins and unpins a group's board, keeping the identity of untouched groups", () => {
    const base = tagState();
    const pinned = withGroupSidebar(base, 'priority', true);
    expect(pinned.groups.filter((g) => g.showInSidebar).map((g) => g.id)).toEqual(['priority']);
    expect(pinned.groups[0]).toBe(base.groups[0]);
    // A no-op keeps every group object.
    expect(withGroupSidebar(pinned, 'priority', true).groups[1]).toBe(pinned.groups[1]);
    const off = withGroupSidebar(pinned, 'priority', false);
    expect(off.groups.some((g) => g.showInSidebar)).toBe(false);
  });

  it('numbers groups in the given order', () => {
    const next = withGroupOrder(tagState(), ['priority', 'status']);
    expect(Object.fromEntries(next.groups.map((g) => [g.id, g.position]))).toEqual({
      priority: 0,
      status: 1,
    });
  });
});

describe('withoutGroup', () => {
  const assignments = [
    { projectId: 'a', tagId: 'doing' },
    { projectId: 'a', tagId: 'q4' },
  ];

  it('deletes the group with its tags and their assignments', () => {
    const next = withoutGroup(tagState(assignments), 'status', 'deleteTags');
    expect(next.groups.map((g) => [g.id, g.position])).toEqual([['priority', 0]]);
    expect(next.tags.some((t) => t.groupId === 'status')).toBe(false);
    expect(next.assignments).toEqual([{ projectId: 'a', tagId: 'q4' }]);
  });

  it('ungroups the tags after the ungrouped ones, keeping assignments', () => {
    const next = withoutGroup(tagState(assignments), 'status', 'ungroup');
    expect(next.groups.map((g) => g.id)).toEqual(['priority']);
    expect(scopeIds(next, null)).toEqual(['q4', 'infra', 'idea', 'doing', 'done']);
    expect(positions(next, null)).toEqual([0, 1, 2, 3, 4]);
    expect(next.assignments).toEqual(assignments);
  });

  it('renames an ungrouped tag whose name is taken to "Name (Group)", then numbers it', () => {
    const state = tagState();
    state.tags.push(
      tag('loose-done', null, 2, { name: 'done' }),
      tag('loose-done-status', null, 3, { name: 'Done (Status)' })
    );
    const next = withoutGroup(state, 'status', 'ungroup');
    const names = Object.fromEntries(next.tags.map((t) => [t.id, t.name]));
    expect(names.idea).toBe('Idea');
    expect(names.done).toBe('Done (Status) 2');
  });
});
