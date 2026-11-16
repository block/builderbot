import { describe, expect, it } from 'vitest';
import type { Tag, TagGroup, TagState } from '../../types';
import {
  applyIntents,
  byPosition,
  chipTags,
  currentIntent,
  effectiveProjectTagIds,
  groupTag,
  indexAssignments,
  indexTagCatalog,
  intentSlot,
  limitChips,
  MAX_TAG_NAME_CHARS,
  numberedName,
  pruneConfirmedIntents,
  reconcileTagState,
  tagNameKey,
  usageCounts,
  withConfirmedIntent,
  withIntent,
  withoutIntent,
  type IntentOverlay,
  type PendingTagIntent,
  type TagIntent,
  type TagSlot,
} from './tagState';

// ── Fixtures ──

function group(id: string, position: number, overrides: Partial<TagGroup> = {}): TagGroup {
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

function tag(
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

/** Status (the icon group) and Priority, plus two ungrouped tags. Out of order on purpose. */
function state(assignments: TagState['assignments'] = []): TagState {
  return {
    groups: [group('priority', 1), group('status', 0, { isIconGroup: true })],
    tags: [
      tag('done', 'status', 1),
      tag('idea', 'status', 0),
      tag('p1', 'priority', 1),
      tag('p0', 'priority', 0),
      tag('q4', null, 1),
      tag('infra', null, 0),
    ],
    assignments,
  };
}

function pending(
  intent: TagIntent,
  token: number,
  confirmedAtSeq: number | null = null
): PendingTagIntent {
  return { intent, token, confirmedAtSeq };
}

const catalog = indexTagCatalog(state());
const ids = (set: ReadonlySet<string> | undefined) => [...(set ?? [])].sort();

// ── Catalog ──

describe('indexTagCatalog', () => {
  it('orders groups and each scope by position, and finds the icon group', () => {
    expect(catalog.groups.map((g) => g.id)).toEqual(['status', 'priority']);
    expect(catalog.tagsByGroup.get('status')!.map((t) => t.id)).toEqual(['idea', 'done']);
    expect(catalog.tagsByGroup.get('priority')!.map((t) => t.id)).toEqual(['p0', 'p1']);
    expect(catalog.tagsByGroup.get(null)!.map((t) => t.id)).toEqual(['infra', 'q4']);
    expect(catalog.iconGroup?.id).toBe('status');
    expect(catalog.tagsById.get('p1')?.groupId).toBe('priority');
  });

  it('keeps the icon group at its stored position, wherever that is', () => {
    // Every surface that lists groups reads `groups`, so the icon group gets
    // no special place: it is found, not moved.
    const reordered = indexTagCatalog({
      ...state(),
      groups: [group('status', 1, { isIconGroup: true }), group('area', 2), group('priority', 0)],
    });
    expect(reordered.groups.map((g) => g.id)).toEqual(['priority', 'status', 'area']);
    expect(reordered.iconGroup?.id).toBe('status');
  });

  it('lists the groups whose boards are pinned to the sidebar, in group order', () => {
    expect(catalog.sidebarGroups).toEqual([]);
    const base = state();
    const pinned = indexTagCatalog({
      ...base,
      groups: base.groups.map((g) => ({ ...g, showInSidebar: true })),
    });
    expect(pinned.sidebarGroups.map((g) => g.id)).toEqual(['status', 'priority']);
  });

  it('breaks position ties by creation time', () => {
    const tied = indexTagCatalog({
      groups: [],
      tags: [tag('b', null, 0, { createdAt: 2 }), tag('a', null, 0, { createdAt: 1 })],
      assignments: [],
    });
    expect(tied.tagsByGroup.get(null)!.map((t) => t.id)).toEqual(['a', 'b']);
  });

  it('breaks position and creation ties by id, as the backend does', () => {
    const tied = indexTagCatalog({
      groups: [group('g-c', 0), group('g-a', 0), group('g-b', 0)],
      tags: [tag('t-c', null, 0), tag('t-a', null, 0), tag('t-b', null, 0)],
      assignments: [],
    });
    expect(tied.groups.map((g) => g.id)).toEqual(['g-a', 'g-b', 'g-c']);
    expect(tied.tagsByGroup.get(null)!.map((t) => t.id)).toEqual(['t-a', 't-b', 't-c']);
  });

  it('compares ids by code unit, as SQLite compares text, not by locale', () => {
    // A locale-aware compare puts "a" before "B"; SQLite's BINARY collation,
    // and so the backend's `ORDER BY`, puts every capital first.
    expect(byPosition(tag('B', null, 0), tag('a', null, 0))).toBeLessThan(0);
    expect(byPosition(tag('a', null, 0), tag('B', null, 0))).toBeGreaterThan(0);
    expect(byPosition(tag('a', null, 0), tag('a', null, 0))).toBe(0);
  });

  it('gives every group an entry and reports no icon group when none is flagged', () => {
    const plain = indexTagCatalog({ groups: [group('empty', 0)], tags: [], assignments: [] });
    expect(plain.tagsByGroup.get('empty')).toEqual([]);
    expect(plain.tagsByGroup.get(null)).toEqual([]);
    expect(plain.iconGroup).toBeNull();
  });

  it('treats a tag whose group is missing as unknown', () => {
    const orphaned = indexTagCatalog({
      groups: [],
      tags: [tag('lost', 'gone', 0)],
      assignments: [],
    });
    expect(orphaned.tagsById.has('lost')).toBe(false);
  });
});

describe('indexAssignments', () => {
  it('groups assignments by project and drops unknown tag ids', () => {
    const byProject = indexAssignments(
      state([
        { projectId: 'a', tagId: 'idea' },
        { projectId: 'a', tagId: 'q4' },
        { projectId: 'a', tagId: 'deleted' },
        { projectId: 'b', tagId: 'deleted' },
      ]),
      catalog.tagsById
    );
    expect(ids(byProject.get('a'))).toEqual(['idea', 'q4']);
    expect(byProject.has('b')).toBe(false);
  });
});

// ── Snapshot reconciliation ──

describe('reconcileTagState', () => {
  const ASSIGNMENTS = [
    { projectId: 'a', tagId: 'idea' },
    { projectId: 'a', tagId: 'q4' },
    { projectId: 'b', tagId: 'idea' },
    { projectId: 'c', tagId: 'done' },
  ];

  /** The snapshot held: frozen, so reconciling can't have edited it. */
  function held(): TagState {
    const current = state(ASSIGNMENTS.map((assignment) => ({ ...assignment })));
    for (const list of [current.groups, current.tags, current.assignments]) {
      list.forEach((row) => Object.freeze(row));
      Object.freeze(list);
    }
    return Object.freeze(current);
  }

  /** What a fetch returns for `current` after `edit`: new objects throughout. */
  function fetchedFrom(current: TagState, edit: (draft: TagState) => void = () => {}): TagState {
    const draft = structuredClone(current);
    edit(draft);
    return draft;
  }

  /**
   * Reconcile, checking what holds for every input: the result equals the
   * fetched snapshot value for value and in its order, and the fetched
   * snapshot is left as it was.
   */
  function reconcile(current: TagState, fetched: TagState): TagState {
    const asFetched = structuredClone(fetched);
    const next = reconcileTagState(current, fetched);
    expect(next).toEqual(asFetched);
    expect(fetched).toEqual(asFetched);
    return next;
  }

  const byId = <T extends { id: string }>(rows: T[], id: string) =>
    rows.find((row) => row.id === id)!;
  const assignmentOf = (rows: TagState['assignments'], projectId: string, tagId: string) =>
    rows.find((row) => row.projectId === projectId && row.tagId === tagId)!;

  /** Which of the three lists `next` took from `current` as they were. */
  function keptLists(current: TagState, next: TagState) {
    return {
      groups: next.groups === current.groups,
      tags: next.tags === current.tags,
      assignments: next.assignments === current.assignments,
    };
  }

  describe('nothing changed', () => {
    it('returns the snapshot held, not a copy', () => {
      const current = held();
      expect(reconcile(current, fetchedFrom(current))).toBe(current);
    });

    it('returns the snapshot held when both are empty', () => {
      const current: TagState = { groups: [], tags: [], assignments: [] };
      expect(reconcile(current, { groups: [], tags: [], assignments: [] })).toBe(current);
    });

    it('returns the snapshot held when given it back', () => {
      const current = held();
      expect(reconcile(current, current)).toBe(current);
    });
  });

  describe('tags', () => {
    it.each([
      ['name', { name: 'Shipped' }],
      ['colour', { color: 'green' }],
      ['icon', { icon: 'check' }],
      ['position', { position: 7 }],
      ['group', { groupId: 'priority' }],
      ['group, to ungrouped', { groupId: null }],
      ['creation time', { createdAt: 5 }],
      ['update time', { updatedAt: 5 }],
    ] as const)('takes the fetched tag when its %s changed, and keeps the rest', (_, change) => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        Object.assign(byId(draft.tags, 'done'), change);
      });
      const next = reconcile(current, fetched);

      expect(byId(next.tags, 'done')).toBe(byId(fetched.tags, 'done'));
      for (const id of ['idea', 'p1', 'p0', 'q4', 'infra']) {
        expect(byId(next.tags, id)).toBe(byId(current.tags, id));
      }
      expect(keptLists(current, next)).toEqual({ groups: true, tags: false, assignments: true });
    });

    it('takes the fetched tag when a field was added to it or dropped from it', () => {
      const current = held();
      const gained = fetchedFrom(current, (draft) => {
        Object.assign(byId(draft.tags, 'done'), { archived: false });
      });
      expect(byId(reconcile(current, gained).tags, 'done')).toBe(byId(gained.tags, 'done'));
      // And the other way round: the held tag carries a field the fetch lacks.
      const lost = fetchedFrom(current);
      expect(byId(reconcile(gained, lost).tags, 'done')).toBe(byId(lost.tags, 'done'));
    });

    it('tells a field that is null from one that is missing', () => {
      const current = held();
      expect(byId(current.tags, 'done').icon).toBeNull();
      const fetched = fetchedFrom(current, (draft) => {
        delete (byId(draft.tags, 'done') as Partial<Tag>).icon;
      });
      expect(byId(reconcile(current, fetched).tags, 'done')).toBe(byId(fetched.tags, 'done'));
    });

    it('adds a new tag and keeps every existing one', () => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.tags.splice(2, 0, tag('blocked', 'status', 2));
      });
      const next = reconcile(current, fetched);

      expect(byId(next.tags, 'blocked')).toBe(byId(fetched.tags, 'blocked'));
      for (const existing of current.tags) expect(byId(next.tags, existing.id)).toBe(existing);
      expect(keptLists(current, next)).toEqual({ groups: true, tags: false, assignments: true });
    });

    it.each([
      ['first', 'done'],
      ['last', 'infra'],
    ])('drops a removed tag (the %s) and keeps the others', (_, removed) => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.tags = draft.tags.filter((t) => t.id !== removed);
      });
      const next = reconcile(current, fetched);

      expect(next.tags).toHaveLength(current.tags.length - 1);
      for (const kept of next.tags) expect(kept).toBe(byId(current.tags, kept.id));
      expect(keptLists(current, next)).toEqual({ groups: true, tags: false, assignments: true });
    });

    it('follows the fetched order when tags only swapped places, keeping every object', () => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.tags.reverse();
      });
      const next = reconcile(current, fetched);

      expect(next.tags.map((t) => t.id)).toEqual(fetched.tags.map((t) => t.id));
      for (const kept of next.tags) expect(kept).toBe(byId(current.tags, kept.id));
      expect(keptLists(current, next)).toEqual({ groups: true, tags: false, assignments: true });
    });

    it('gives a reordered scope new objects for the tags whose position moved only', () => {
      // What a real reorder fetches: idea and done swap positions in Status.
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        byId(draft.tags, 'idea').position = 1;
        byId(draft.tags, 'done').position = 0;
      });
      const next = reconcile(current, fetched);

      for (const id of ['idea', 'done']) {
        expect(byId(next.tags, id)).toBe(byId(fetched.tags, id));
      }
      for (const id of ['p1', 'p0', 'q4', 'infra']) {
        expect(byId(next.tags, id)).toBe(byId(current.tags, id));
      }
    });
  });

  describe('groups', () => {
    it.each([
      ['name', { name: 'Stage' }],
      ['position', { position: 4 }],
      ['icon flag', { isIconGroup: false }],
      ['sidebar flag', { showInSidebar: true }],
      ['creation time', { createdAt: 5 }],
      ['update time', { updatedAt: 5 }],
    ] as const)('takes the fetched group when its %s changed, and keeps the rest', (_, change) => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        Object.assign(byId(draft.groups, 'status'), change);
      });
      const next = reconcile(current, fetched);

      expect(byId(next.groups, 'status')).toBe(byId(fetched.groups, 'status'));
      expect(byId(next.groups, 'priority')).toBe(byId(current.groups, 'priority'));
      expect(keptLists(current, next)).toEqual({ groups: false, tags: true, assignments: true });
    });

    it('adds a new group and keeps every existing one', () => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.groups.unshift(group('area', 2));
      });
      const next = reconcile(current, fetched);

      expect(byId(next.groups, 'area')).toBe(byId(fetched.groups, 'area'));
      for (const existing of current.groups) expect(byId(next.groups, existing.id)).toBe(existing);
      expect(keptLists(current, next)).toEqual({ groups: false, tags: true, assignments: true });
    });

    it.each([
      ['first', 'priority'],
      ['last', 'status'],
    ])('drops a removed group (the %s) and keeps the other', (_, removed) => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.groups = draft.groups.filter((g) => g.id !== removed);
      });
      const next = reconcile(current, fetched);

      expect(next.groups).toHaveLength(1);
      expect(next.groups[0]).toBe(byId(current.groups, next.groups[0].id));
      expect(keptLists(current, next)).toEqual({ groups: false, tags: true, assignments: true });
    });

    it('follows the fetched order when groups only swapped places, keeping every object', () => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.groups.reverse();
      });
      const next = reconcile(current, fetched);

      expect(next.groups.map((g) => g.id)).toEqual(['status', 'priority']);
      for (const kept of next.groups) expect(kept).toBe(byId(current.groups, kept.id));
      expect(keptLists(current, next)).toEqual({ groups: false, tags: true, assignments: true });
    });

    it('gives reordered groups new objects, since their positions changed', () => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        byId(draft.groups, 'status').position = 1;
        byId(draft.groups, 'priority').position = 0;
      });
      const next = reconcile(current, fetched);

      for (const id of ['status', 'priority']) {
        expect(byId(next.groups, id)).toBe(byId(fetched.groups, id));
      }
      expect(keptLists(current, next)).toEqual({ groups: false, tags: true, assignments: true });
    });
  });

  describe('assignments', () => {
    it('replaces a retagged project’s assignment and keeps every other project’s', () => {
      // A card moved to another column: b goes from idea to done.
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        assignmentOf(draft.assignments, 'b', 'idea').tagId = 'done';
      });
      const next = reconcile(current, fetched);

      expect(assignmentOf(next.assignments, 'b', 'done')).toBe(
        assignmentOf(fetched.assignments, 'b', 'done')
      );
      expect(assignmentOf(next.assignments, 'b', 'idea')).toBeUndefined();
      for (const [projectId, tagId] of [
        ['a', 'idea'],
        ['a', 'q4'],
        ['c', 'done'],
      ]) {
        expect(assignmentOf(next.assignments, projectId, tagId)).toBe(
          assignmentOf(current.assignments, projectId, tagId)
        );
      }
      expect(keptLists(current, next)).toEqual({ groups: true, tags: true, assignments: false });
    });

    it('adds a new assignment and keeps every existing one', () => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.assignments.splice(1, 0, { projectId: 'a', tagId: 'p0' });
      });
      const next = reconcile(current, fetched);

      expect(assignmentOf(next.assignments, 'a', 'p0')).toBe(
        assignmentOf(fetched.assignments, 'a', 'p0')
      );
      for (const existing of current.assignments) {
        expect(assignmentOf(next.assignments, existing.projectId, existing.tagId)).toBe(existing);
      }
      expect(keptLists(current, next)).toEqual({ groups: true, tags: true, assignments: false });
    });

    it.each([
      ['first', 0],
      ['last', ASSIGNMENTS.length - 1],
    ])('drops a removed assignment (the %s) and keeps the others', (_, index) => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.assignments.splice(index, 1);
      });
      const next = reconcile(current, fetched);

      expect(next.assignments).toHaveLength(ASSIGNMENTS.length - 1);
      for (const kept of next.assignments) {
        expect(kept).toBe(assignmentOf(current.assignments, kept.projectId, kept.tagId));
      }
      expect(keptLists(current, next)).toEqual({ groups: true, tags: true, assignments: false });
    });

    it('follows the fetched order when assignments only swapped places, keeping every object', () => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.assignments.reverse();
      });
      const next = reconcile(current, fetched);

      expect(next.assignments).toEqual([...ASSIGNMENTS].reverse());
      for (const kept of next.assignments) {
        expect(kept).toBe(assignmentOf(current.assignments, kept.projectId, kept.tagId));
      }
      expect(keptLists(current, next)).toEqual({ groups: true, tags: true, assignments: false });
    });

    it('tells apart assignments whose project and tag ids run together the same', () => {
      const current: TagState = {
        groups: [],
        tags: [],
        assignments: [
          { projectId: 'ab', tagId: 'c' },
          { projectId: 'a', tagId: 'bc' },
        ],
      };
      expect(reconcile(current, fetchedFrom(current))).toBe(current);
    });

    it('takes the fetched assignment when a field was added to it', () => {
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        Object.assign(assignmentOf(draft.assignments, 'c', 'done'), { pinned: true });
      });
      const next = reconcile(current, fetched);
      expect(assignmentOf(next.assignments, 'c', 'done')).toBe(
        assignmentOf(fetched.assignments, 'c', 'done')
      );
      expect(assignmentOf(next.assignments, 'a', 'idea')).toBe(
        assignmentOf(current.assignments, 'a', 'idea')
      );
    });
  });

  describe('several lists at once', () => {
    it('reconciles each list on its own', () => {
      // Deleting a tag: it goes, its assignments go, its scope is renumbered.
      const current = held();
      const fetched = fetchedFrom(current, (draft) => {
        draft.tags = draft.tags.filter((t) => t.id !== 'idea');
        byId(draft.tags, 'done').position = 0;
        draft.assignments = draft.assignments.filter((a) => a.tagId !== 'idea');
      });
      const next = reconcile(current, fetched);

      expect(byId(next.tags, 'done')).toBe(byId(fetched.tags, 'done'));
      expect(byId(next.tags, 'q4')).toBe(byId(current.tags, 'q4'));
      expect(next.assignments).toEqual([
        { projectId: 'a', tagId: 'q4' },
        { projectId: 'c', tagId: 'done' },
      ]);
      expect(assignmentOf(next.assignments, 'a', 'q4')).toBe(
        assignmentOf(current.assignments, 'a', 'q4')
      );
      expect(keptLists(current, next)).toEqual({ groups: true, tags: false, assignments: false });
    });

    it('takes everything from the fetch when nothing is held', () => {
      const fetched = fetchedFrom(held());
      const next = reconcile({ groups: [], tags: [], assignments: [] }, fetched);
      expect(next.groups.every((g, i) => g === fetched.groups[i])).toBe(true);
      expect(next.tags.every((t, i) => t === fetched.tags[i])).toBe(true);
      expect(next.assignments.every((a, i) => a === fetched.assignments[i])).toBe(true);
    });

    it('empties every list when the fetch is empty', () => {
      const current = held();
      const next = reconcile(current, { groups: [], tags: [], assignments: [] });
      expect(next).not.toBe(current);
      expect(keptLists(current, next)).toEqual({ groups: false, tags: false, assignments: false });
    });
  });
});

// ── Intents ──

describe('intentSlot', () => {
  it('keys group writes by group and ungrouped writes by tag', () => {
    expect(intentSlot({ kind: 'group', groupId: 'status', tagId: null })).toBe('g:status');
    expect(intentSlot({ kind: 'tag', tagId: 'q4', assigned: true })).toBe('t:q4');
  });
});

describe('applyIntents', () => {
  const server = new Set(['idea', 'p0', 'infra']);
  const apply = (...intents: TagIntent[]) =>
    ids(
      applyIntents(
        server,
        new Map(intents.map((intent, i) => [intentSlot(intent), pending(intent, i)])),
        catalog.tagsById
      )
    );

  it('returns the server set when nothing is pending', () => {
    expect(ids(applyIntents(server, undefined, catalog.tagsById))).toEqual(ids(server));
  });

  it('replaces the group’s tag with the chosen one', () => {
    expect(apply({ kind: 'group', groupId: 'status', tagId: 'done' })).toEqual([
      'done',
      'infra',
      'p0',
    ]);
  });

  it('clears the group with a null choice and leaves other groups alone', () => {
    expect(apply({ kind: 'group', groupId: 'status', tagId: null })).toEqual(['infra', 'p0']);
  });

  it('adds and removes ungrouped tags', () => {
    expect(
      apply(
        { kind: 'tag', tagId: 'q4', assigned: true },
        { kind: 'tag', tagId: 'infra', assigned: false }
      )
    ).toEqual(['idea', 'p0', 'q4']);
  });

  it('ignores a pick of a tag that is unknown or has left the group', () => {
    expect(apply({ kind: 'group', groupId: 'status', tagId: 'p1' })).toEqual(['infra', 'p0']);
    expect(apply({ kind: 'tag', tagId: 'deleted', assigned: true })).toEqual(ids(server));
  });

  it('does not touch the server set it was given', () => {
    apply({ kind: 'group', groupId: 'status', tagId: null });
    expect(ids(server)).toEqual(['idea', 'infra', 'p0']);
  });
});

describe('effectiveProjectTagIds', () => {
  it('applies each project’s intents and shares the server sets of the rest', () => {
    const server = new Map<string, ReadonlySet<string>>([
      ['a', new Set(['idea'])],
      ['b', new Set(['done'])],
    ]);
    const overlay: IntentOverlay = new Map([
      [
        'a',
        new Map<TagSlot, PendingTagIntent>([
          ['g:status', pending({ kind: 'group', groupId: 'status', tagId: 'done' }, 1)],
        ]),
      ],
      [
        'c',
        new Map<TagSlot, PendingTagIntent>([
          ['t:q4', pending({ kind: 'tag', tagId: 'q4', assigned: true }, 2)],
        ]),
      ],
    ]);
    const effective = effectiveProjectTagIds(server, overlay, catalog.tagsById);
    expect(ids(effective.get('a'))).toEqual(['done']);
    expect(effective.get('b')).toBe(server.get('b'));
    expect(ids(effective.get('c'))).toEqual(['q4']);
  });
});

describe('overlay bookkeeping', () => {
  const pick = (tagId: string | null): TagIntent => ({ kind: 'group', groupId: 'status', tagId });

  it('keeps only the latest intent per slot', () => {
    let overlay: IntentOverlay = new Map();
    overlay = withIntent(overlay, 'a', pending(pick('idea'), 1));
    overlay = withIntent(overlay, 'a', pending(pick('done'), 2));
    overlay = withIntent(overlay, 'a', pending({ kind: 'tag', tagId: 'q4', assigned: true }, 3));
    expect(overlay.get('a')!.size).toBe(2);
    expect(currentIntent(overlay, 'a', 'g:status')?.token).toBe(2);
  });

  it('drops or confirms an intent only for its own token', () => {
    const overlay = withIntent(new Map(), 'a', pending(pick('done'), 2));
    expect(withoutIntent(overlay, 'a', 'g:status', 1)).toBe(overlay);
    expect(withConfirmedIntent(overlay, 'a', 'g:status', 1, 5)).toBe(overlay);

    expect(withoutIntent(overlay, 'a', 'g:status', 2).has('a')).toBe(false);
    const confirmed = withConfirmedIntent(overlay, 'a', 'g:status', 2, 5);
    expect(currentIntent(confirmed, 'a', 'g:status')?.confirmedAtSeq).toBe(5);
    // Pure: the original overlay is untouched.
    expect(currentIntent(overlay, 'a', 'g:status')?.confirmedAtSeq).toBeNull();
  });

  it('prunes confirmed intents only once a snapshot requested after them lands', () => {
    let overlay: IntentOverlay = new Map();
    overlay = withIntent(overlay, 'a', pending(pick('done'), 1, 5));
    overlay = withIntent(overlay, 'a', pending({ kind: 'tag', tagId: 'q4', assigned: true }, 2));
    overlay = withIntent(overlay, 'b', pending(pick('idea'), 3, 7));

    expect(pruneConfirmedIntents(overlay, 5)).toBe(overlay);
    const pruned = pruneConfirmedIntents(overlay, 6);
    // a's confirmed pick goes, its pending write stays; b's is newer than 6.
    expect([...pruned.get('a')!.keys()]).toEqual(['t:q4']);
    expect(pruned.get('b')).toBe(overlay.get('b'));
    expect(pruneConfirmedIntents(pruned, 8).has('b')).toBe(false);
  });
});

// ── Lookups ──

describe('groupTag', () => {
  it('finds the project’s tag in a group, or null', () => {
    const set = new Set(['done', 'p1', 'q4']);
    expect(groupTag(set, 'status', catalog.tagsById)?.id).toBe('done');
    expect(groupTag(set, 'nope', catalog.tagsById)).toBeNull();
    expect(groupTag(undefined, 'status', catalog.tagsById)).toBeNull();
  });

  it('picks the first by position if two ever show up', () => {
    expect(groupTag(new Set(['done', 'idea']), 'status', catalog.tagsById)?.id).toBe('idea');
  });
});

describe('chipTags', () => {
  const set = new Set(['q4', 'p1', 'done', 'infra']);

  it('orders chips by group order, then ungrouped by position', () => {
    expect(chipTags(set, catalog).map((t) => t.id)).toEqual(['done', 'p1', 'infra', 'q4']);
  });

  it('skips excluded groups and tolerates null entries', () => {
    expect(
      chipTags(set, catalog, { excludeGroupIds: ['status', null, undefined] }).map((t) => t.id)
    ).toEqual(['p1', 'infra', 'q4']);
  });

  it('is empty without tags', () => {
    expect(chipTags(undefined, catalog)).toEqual([]);
  });

  it('gives a grid card the tags outside the icon group, in order', () => {
    const all = new Set(['infra', 'q4', 'p0', 'idea']);
    expect(chipTags(all, catalog, { excludeGroupIds: ['status'] }).map((t) => t.id)).toEqual([
      'p0',
      'infra',
      'q4',
    ]);
  });
});

describe('limitChips', () => {
  const chips = chipTags(new Set(['done', 'p1', 'infra', 'q4']), catalog);

  it('keeps the first chips in order and counts the rest', () => {
    const { shown, overflow } = limitChips(chips, 3);
    expect(shown.map((t) => t.id)).toEqual(['done', 'p1', 'infra']);
    expect(overflow).toBe(1);
  });

  it('shows every chip when they fit', () => {
    expect(limitChips(chips, 4)).toEqual({ shown: chips, overflow: 0 });
    expect(limitChips([], 3)).toEqual({ shown: [], overflow: 0 });
  });
});

describe('usageCounts', () => {
  it('counts each tag over the given projects only', () => {
    const byProject = new Map<string, ReadonlySet<string>>([
      ['a', new Set(['idea', 'q4'])],
      ['b', new Set(['idea'])],
      ['deleting', new Set(['idea', 'infra'])],
    ]);
    const counts = usageCounts(byProject, ['a', 'b', 'c']);
    expect(Object.fromEntries(counts)).toEqual({ idea: 2, q4: 1 });
  });
});

describe('tagNameKey', () => {
  it('folds ASCII case only, as SQLite NOCASE does', () => {
    expect(tagNameKey('Infra')).toBe(tagNameKey('infra'));
    expect(tagNameKey('É')).not.toBe(tagNameKey('é'));
  });

  it('ignores surrounding whitespace, which the backend trims', () => {
    expect(tagNameKey('  High ')).toBe('high');
  });
});

describe('numberedName', () => {
  it('keeps a free name', () => {
    expect(numberedName('Status', new Set(['priority']))).toBe('Status');
  });

  it('numbers a taken name from 2, ignoring case', () => {
    expect(numberedName('Status', new Set(['status']))).toBe('Status 2');
    expect(numberedName('Status', new Set(['status', 'status 2']))).toBe('Status 3');
  });

  it('cuts the base so the number still fits the name limit', () => {
    const base = 'x'.repeat(MAX_TAG_NAME_CHARS);
    const name = numberedName(base, new Set([base]));
    expect(name).toBe(`${'x'.repeat(MAX_TAG_NAME_CHARS - 2)} 2`);
    expect(name).toHaveLength(MAX_TAG_NAME_CHARS);
  });
});
