import { describe, expect, it } from 'vitest';
import type { TagState } from '../../types';
import { resolveGroupDrop, resolveTagDrop, stepTarget, steppedGroupOrder } from './tagReorder';
import { indexTagCatalog } from './tagState';
import { group, tag, tagState } from './testStubs/tagFixtures';

const catalog = indexTagCatalog(tagState());

describe('resolveTagDrop: within a scope', () => {
  it('moves a tag down, after its anchor', () => {
    expect(
      resolveTagDrop(catalog, 'idea', { groupId: 'status', anchorTagId: 'doing', place: 'after' })
    ).toEqual({
      tagId: 'idea',
      groupId: 'status',
      index: 1,
      crossScope: false,
      conflicts: 0,
      collision: null,
    });
  });

  it('moves a tag up, before its anchor', () => {
    expect(
      resolveTagDrop(catalog, 'done', { groupId: 'status', anchorTagId: 'idea', place: 'before' })
    ).toMatchObject({ groupId: 'status', index: 0, crossScope: false });
  });

  it('moves a tag to the end of its scope', () => {
    expect(resolveTagDrop(catalog, 'idea', { groupId: 'status', anchorTagId: null })).toMatchObject(
      { index: 2 }
    );
  });

  it('is a no-op when the tag would land where it is', () => {
    // Just below the tag above it, or just above the tag below it.
    expect(
      resolveTagDrop(catalog, 'doing', { groupId: 'status', anchorTagId: 'idea', place: 'after' })
    ).toBeNull();
    expect(
      resolveTagDrop(catalog, 'doing', { groupId: 'status', anchorTagId: 'done', place: 'before' })
    ).toBeNull();
    expect(resolveTagDrop(catalog, 'done', { groupId: 'status', anchorTagId: null })).toBeNull();
  });

  it('is a no-op when dropped on itself', () => {
    expect(
      resolveTagDrop(catalog, 'doing', { groupId: 'status', anchorTagId: 'doing', place: 'after' })
    ).toBeNull();
  });
});

describe('resolveTagDrop: catalog changed mid-drag', () => {
  it('is a no-op when the dragged tag was deleted', () => {
    expect(resolveTagDrop(catalog, 'gone', { groupId: 'status', anchorTagId: null })).toBeNull();
  });

  it('is a no-op when the target group was deleted', () => {
    expect(resolveTagDrop(catalog, 'q4', { groupId: 'gone', anchorTagId: null })).toBeNull();
  });

  it('is a no-op when the anchor left the target scope', () => {
    // p0 is in Priority, not Status: the target was recorded before it moved.
    expect(
      resolveTagDrop(catalog, 'idea', { groupId: 'status', anchorTagId: 'p0', place: 'before' })
    ).toBeNull();
  });

  it('resolves against the list at drop time', () => {
    // "doing" was deleted after the drag started; "after idea" is now index 1 of [idea, done].
    const shrunk: TagState = {
      ...tagState(),
      tags: tagState().tags.filter((t) => t.id !== 'doing'),
    };
    expect(
      resolveTagDrop(indexTagCatalog(shrunk), 'q4', {
        groupId: 'status',
        anchorTagId: 'idea',
        place: 'after',
      })
    ).toMatchObject({ groupId: 'status', index: 1 });
  });
});

describe('resolveTagDrop: across scopes', () => {
  it('moves an ungrouped tag into a group beside its anchor', () => {
    expect(
      resolveTagDrop(catalog, 'q4', { groupId: 'status', anchorTagId: 'doing', place: 'before' })
    ).toEqual({
      tagId: 'q4',
      groupId: 'status',
      index: 1,
      crossScope: true,
      conflicts: 0,
      collision: null,
    });
  });

  it('moves a grouped tag out to the ungrouped tags', () => {
    expect(resolveTagDrop(catalog, 'p1', { groupId: null, anchorTagId: null })).toMatchObject({
      groupId: null,
      index: 2,
      crossScope: true,
      conflicts: 0,
    });
  });

  it('counts the projects that already have a tag in the target group', () => {
    const projects = [
      new Set(['q4', 'doing']), // has a Status: keeps it, loses q4
      new Set(['q4', 'p0']), // no Status: q4 becomes it
      new Set(['done']), // doesn't carry q4
      new Set(['q4', 'idea', 'infra']), // has a Status
    ];
    expect(
      resolveTagDrop(catalog, 'q4', { groupId: 'status', anchorTagId: null }, projects)
    ).toMatchObject({ conflicts: 2 });
  });

  it('counts conflicts for a move between groups too', () => {
    const projects = [new Set(['p0', 'idea']), new Set(['p0'])];
    expect(
      resolveTagDrop(catalog, 'p0', { groupId: 'status', anchorTagId: null }, projects)
    ).toMatchObject({ conflicts: 1 });
  });

  it('has no conflicts moving out to the ungrouped tags', () => {
    const projects = [new Set(['p0', 'infra'])];
    expect(
      resolveTagDrop(catalog, 'p0', { groupId: null, anchorTagId: null }, projects)
    ).toMatchObject({ conflicts: 0 });
  });

  it('reports a name the target scope already has, ignoring case', () => {
    const state = tagState();
    state.tags.push(tag('urgent', null, 2, { name: 'DONE' }));
    const move = resolveTagDrop(indexTagCatalog(state), 'urgent', {
      groupId: 'status',
      anchorTagId: null,
    });
    expect(move?.collision?.id).toBe('done');
  });
});

describe('stepTarget', () => {
  it('targets the tag above, or below', () => {
    expect(stepTarget(catalog, 'doing', -1)).toEqual({
      groupId: 'status',
      anchorTagId: 'idea',
      place: 'before',
    });
    expect(stepTarget(catalog, 'doing', 1)).toEqual({
      groupId: 'status',
      anchorTagId: 'done',
      place: 'after',
    });
  });

  it('resolves to a one-step move', () => {
    const target = stepTarget(catalog, 'doing', 1)!;
    expect(resolveTagDrop(catalog, 'doing', target)).toMatchObject({ index: 2 });
  });

  it('is null at either end of the scope', () => {
    expect(stepTarget(catalog, 'idea', -1)).toBeNull();
    expect(stepTarget(catalog, 'done', 1)).toBeNull();
    expect(stepTarget(catalog, 'infra', 1)).toBeNull();
  });

  it('is null for an unknown tag', () => {
    expect(stepTarget(catalog, 'gone', 1)).toBeNull();
  });
});

describe('steppedGroupOrder', () => {
  it('swaps a group with its neighbour', () => {
    expect(steppedGroupOrder(catalog.groups, 'priority', -1)).toEqual(['priority', 'status']);
    expect(steppedGroupOrder(catalog.groups, 'status', 1)).toEqual(['priority', 'status']);
  });

  it('is null at either end, or for an unknown group', () => {
    expect(steppedGroupOrder(catalog.groups, 'status', -1)).toBeNull();
    expect(steppedGroupOrder(catalog.groups, 'priority', 1)).toBeNull();
    expect(steppedGroupOrder(catalog.groups, 'gone', 1)).toBeNull();
  });
});

describe('resolveGroupDrop', () => {
  // Status, Priority, Area, Team.
  const groups = indexTagCatalog({
    ...tagState(),
    groups: [...tagState().groups, group('area', 2), group('team', 3)],
  }).groups;

  it('moves a group down, after its anchor', () => {
    expect(resolveGroupDrop(groups, 'status', { anchorGroupId: 'area', place: 'after' })).toEqual([
      'priority',
      'area',
      'status',
      'team',
    ]);
  });

  it('moves a group up, before its anchor', () => {
    expect(
      resolveGroupDrop(groups, 'team', { anchorGroupId: 'priority', place: 'before' })
    ).toEqual(['status', 'team', 'priority', 'area']);
  });

  it('moves a group to either end', () => {
    expect(resolveGroupDrop(groups, 'area', { anchorGroupId: 'status', place: 'before' })).toEqual([
      'area',
      'status',
      'priority',
      'team',
    ]);
    expect(resolveGroupDrop(groups, 'status', { anchorGroupId: 'team', place: 'after' })).toEqual([
      'priority',
      'area',
      'team',
      'status',
    ]);
  });

  it('is a no-op when the group would land where it is', () => {
    // Just below the group above it, or just above the group below it.
    expect(
      resolveGroupDrop(groups, 'priority', { anchorGroupId: 'status', place: 'after' })
    ).toBeNull();
    expect(
      resolveGroupDrop(groups, 'priority', { anchorGroupId: 'area', place: 'before' })
    ).toBeNull();
  });

  it('is a no-op when dropped on itself', () => {
    expect(
      resolveGroupDrop(groups, 'priority', { anchorGroupId: 'priority', place: 'before' })
    ).toBeNull();
    expect(
      resolveGroupDrop(groups, 'priority', { anchorGroupId: 'priority', place: 'after' })
    ).toBeNull();
  });

  it('is a no-op when the dragged group or the anchor was deleted mid-drag', () => {
    expect(resolveGroupDrop(groups, 'gone', { anchorGroupId: 'area', place: 'after' })).toBeNull();
    expect(
      resolveGroupDrop(groups, 'status', { anchorGroupId: 'gone', place: 'after' })
    ).toBeNull();
    expect(resolveGroupDrop([], 'status', { anchorGroupId: 'area', place: 'after' })).toBeNull();
  });

  it('resolves against the list at drop time', () => {
    // "priority" was deleted and "squad" created after the drag started: the
    // order names every group there is now, and no other.
    const changed = [...groups.filter((g) => g.id !== 'priority'), group('squad', 4)];
    expect(resolveGroupDrop(changed, 'team', { anchorGroupId: 'status', place: 'after' })).toEqual([
      'status',
      'team',
      'area',
      'squad',
    ]);
    // "after status" was a move when the drag began, with Priority between them.
    expect(
      resolveGroupDrop(changed, 'area', { anchorGroupId: 'status', place: 'after' })
    ).toBeNull();
  });

  it('leaves the groups it was given untouched', () => {
    const before = groups.map((g) => g.id);
    resolveGroupDrop(groups, 'status', { anchorGroupId: 'team', place: 'after' });
    expect(groups.map((g) => g.id)).toEqual(before);
  });
});
