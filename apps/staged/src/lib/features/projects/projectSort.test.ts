import { describe, expect, it } from 'vitest';
import type { Project, TagState } from '../../types';
import { buildBoardColumns } from '../board/boardColumns';
import { indexAssignments, indexTagCatalog } from '../tags/tagState';
import { tagState } from '../tags/testStubs/tagFixtures';
import {
  DEFAULT_SORT_SUMMARY,
  addableSortKeys,
  defaultSortDirection,
  isInertOnBoard,
  lastUpdatedAt,
  normalizeProjectSort,
  resolveProjectSort,
  sortKeyId,
  sortKeyLabel,
  sortProjects,
  sortSummary,
  sortUsesActivity,
  withSortKeyAdded,
  withSortKeyDirection,
  withSortKeyMoved,
  withoutSortKey,
  type ProjectSort,
  type ProjectSortContext,
  type ProjectSortKey,
} from './projectSort';

// ── Fixtures ──

function project(id: string, createdAt: number): Project {
  return {
    id,
    name: id,
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt,
    updatedAt: createdAt,
  };
}

// Newest first, as the backend lists them.
const a = project('a', 400);
const b = project('b', 300);
const c = project('c', 200);
const d = project('d', 100);
const projects = [a, b, c, d];

// Status (icon group: idea, doing, done), Priority (p0, p1), ungrouped q4, infra.
const ASSIGNMENTS: TagState['assignments'] = [
  { projectId: 'a', tagId: 'p1' },
  { projectId: 'a', tagId: 'doing' },
  { projectId: 'b', tagId: 'done' },
  { projectId: 'c', tagId: 'p0' },
  { projectId: 'c', tagId: 'doing' },
  { projectId: 'd', tagId: 'p1' },
  { projectId: 'd', tagId: 'idea' },
];

/** The sort's context over a tag snapshot, as the store builds it. */
function contextOf(
  state: TagState,
  activity?: ReadonlyMap<string, number> | null
): ProjectSortContext {
  const catalog = indexTagCatalog(state);
  return { catalog, projectTagIds: indexAssignments(state, catalog.tagsById), activity };
}

const state = tagState(ASSIGNMENTS);
const catalog = indexTagCatalog(state);
const ctx = contextOf(state);

const created = (direction: 'asc' | 'desc'): ProjectSortKey => ({ kind: 'created', direction });
const updated = (direction: 'asc' | 'desc'): ProjectSortKey => ({ kind: 'updated', direction });
const byGroup = (groupId: string, direction: 'asc' | 'desc'): ProjectSortKey => ({
  kind: 'group',
  groupId,
  direction,
});

const ids = (list: readonly Project[]) => list.map((p) => p.id);

// ── Keys ──

describe('sortKeyId', () => {
  it('names a key by kind, and a group key by its group, whichever way it points', () => {
    expect(sortKeyId(created('asc'))).toBe('created');
    expect(sortKeyId(created('desc'))).toBe('created');
    expect(sortKeyId(updated('desc'))).toBe('updated');
    expect(sortKeyId(byGroup('priority', 'asc'))).toBe('group:priority');
    expect(sortKeyId(byGroup('status', 'desc'))).toBe('group:status');
  });
});

describe('defaultSortDirection', () => {
  it('starts dates newest first and a group in its tag order', () => {
    expect(defaultSortDirection('created')).toBe('desc');
    expect(defaultSortDirection('updated')).toBe('desc');
    expect(defaultSortDirection('group')).toBe('asc');
  });
});

// ── Reading ──

describe('normalizeProjectSort', () => {
  it('keeps well-formed keys in order', () => {
    const stored = [byGroup('priority', 'asc'), updated('desc'), created('asc')];
    expect(normalizeProjectSort(stored)).toEqual(stored);
  });

  it('reads anything but a list as no sort', () => {
    expect(normalizeProjectSort(undefined)).toEqual([]);
    expect(normalizeProjectSort(null)).toEqual([]);
    expect(normalizeProjectSort('created')).toEqual([]);
    expect(normalizeProjectSort({ kind: 'created', direction: 'desc' })).toEqual([]);
  });

  it('drops malformed entries and unknown kinds', () => {
    expect(
      normalizeProjectSort([
        null,
        'created',
        42,
        ['created'],
        {},
        { kind: 'created' },
        { kind: 'created', direction: 'sideways' },
        { kind: 'name', direction: 'asc' },
        { kind: 'group', direction: 'asc' },
        { kind: 'group', groupId: '', direction: 'asc' },
        { kind: 'group', groupId: 7, direction: 'asc' },
        { kind: 'updated', direction: 'asc' },
      ])
    ).toEqual([updated('asc')]);
  });

  it('keeps the first of two entries for one key', () => {
    expect(
      normalizeProjectSort([
        created('asc'),
        byGroup('priority', 'desc'),
        created('desc'),
        byGroup('status', 'asc'),
        byGroup('priority', 'asc'),
      ])
    ).toEqual([created('asc'), byGroup('priority', 'desc'), byGroup('status', 'asc')]);
  });

  it('drops fields a key doesn’t have', () => {
    expect(
      normalizeProjectSort([
        { kind: 'created', direction: 'asc', groupId: 'priority', extra: true },
        { kind: 'group', groupId: 'status', direction: 'desc', pinned: 1 },
      ])
    ).toEqual([created('asc'), byGroup('status', 'desc')]);
  });

  it('keeps groups the catalog doesn’t know, for the next write to prune', () => {
    expect(normalizeProjectSort([byGroup('gone', 'asc')])).toEqual([byGroup('gone', 'asc')]);
  });
});

describe('resolveProjectSort', () => {
  it('drops keys for groups the catalog no longer has, keeping the rest in order', () => {
    const sort = [byGroup('gone', 'asc'), updated('desc'), byGroup('priority', 'asc')];
    expect(resolveProjectSort(sort, catalog)).toEqual([
      updated('desc'),
      byGroup('priority', 'asc'),
    ]);
  });

  it('returns the sort itself when every key is known', () => {
    const sort = [byGroup('status', 'desc'), created('asc')];
    expect(resolveProjectSort(sort, catalog)).toBe(sort);
    const none: ProjectSort = [];
    expect(resolveProjectSort(none, catalog)).toBe(none);
  });
});

// ── Sorting ──

describe('sortUsesActivity', () => {
  it('is true only with a last-updated key, wherever it sits', () => {
    expect(sortUsesActivity([])).toBe(false);
    expect(
      sortUsesActivity([
        { kind: 'created', direction: 'desc' },
        { kind: 'group', groupId: 'priority', direction: 'asc' },
      ])
    ).toBe(false);
    expect(
      sortUsesActivity([
        { kind: 'group', groupId: 'priority', direction: 'asc' },
        { kind: 'updated', direction: 'asc' },
      ])
    ).toBe(true);
  });
});

describe('sortProjects', () => {
  it('returns the input array itself for an empty sort', () => {
    expect(sortProjects(projects, [], ctx)).toBe(projects);
    // Even with a previous result on hand.
    expect(sortProjects(projects, [], ctx, [d, c, b, a])).toBe(projects);
  });

  it('returns the input array itself when no key names a known group', () => {
    expect(sortProjects(projects, [byGroup('gone', 'asc')], ctx)).toBe(projects);
  });

  it('ignores a key for an unknown group among known ones', () => {
    expect(ids(sortProjects(projects, [byGroup('gone', 'desc'), created('asc')], ctx))).toEqual([
      'd',
      'c',
      'b',
      'a',
    ]);
  });

  it('leaves the input array as it was', () => {
    const input = [a, b, c, d];
    const sorted = sortProjects(input, [created('asc')], ctx);
    expect(sorted).not.toBe(input);
    expect(input).toEqual([a, b, c, d]);
  });

  it('sorts by created date in both directions', () => {
    const shuffled = [c, a, d, b];
    expect(ids(sortProjects(shuffled, [created('desc')], ctx))).toEqual(['a', 'b', 'c', 'd']);
    expect(ids(sortProjects(shuffled, [created('asc')], ctx))).toEqual(['d', 'c', 'b', 'a']);
  });

  it('sorts by last updated in both directions', () => {
    const activity = new Map([
      ['a', 500],
      ['b', 900],
      ['c', 700],
      ['d', 600],
    ]);
    const withActivity = contextOf(state, activity);
    expect(ids(sortProjects(projects, [updated('desc')], withActivity))).toEqual([
      'b',
      'c',
      'd',
      'a',
    ]);
    expect(ids(sortProjects(projects, [updated('asc')], withActivity))).toEqual([
      'a',
      'd',
      'c',
      'b',
    ]);
  });

  it('falls back to the created date for a project without activity', () => {
    // d was updated after b and c were created; a, b and c have no activity.
    const withActivity = contextOf(state, new Map([['d', 350]]));
    expect(ids(sortProjects(projects, [updated('desc')], withActivity))).toEqual([
      'a',
      'd',
      'b',
      'c',
    ]);
  });

  it('sorts by created date when there is no activity at all', () => {
    for (const activity of [undefined, null, new Map<string, number>()]) {
      const without = contextOf(state, activity);
      expect(ids(sortProjects(projects, [updated('asc')], without))).toEqual(['d', 'c', 'b', 'a']);
    }
  });

  it('never counts a project as updated before it was created', () => {
    // Stale activity older than the project (a branch moved in from an
    // older one, say) doesn't sink it below its creation.
    const withActivity = contextOf(state, new Map([['a', 50]]));
    expect(ids(sortProjects(projects, [updated('desc')], withActivity))).toEqual([
      'a',
      'b',
      'c',
      'd',
    ]);
  });

  it('sorts by a group in its tag order, untagged last', () => {
    // Priority: c is p0; a and d are p1; b has none.
    expect(ids(sortProjects(projects, [byGroup('priority', 'asc')], ctx))).toEqual([
      'c',
      'a',
      'd',
      'b',
    ]);
    // Status: d is idea; a and c are doing; b is done.
    expect(ids(sortProjects(projects, [byGroup('status', 'asc')], ctx))).toEqual([
      'd',
      'a',
      'c',
      'b',
    ]);
  });

  it('keeps untagged projects last when the group key is reversed', () => {
    expect(ids(sortProjects(projects, [byGroup('priority', 'desc')], ctx))).toEqual([
      'a',
      'd',
      'c',
      'b',
    ]);
  });

  it('keeps the input order among projects a key ties, in both directions', () => {
    const e = project('e', 300);
    const f = project('f', 300);
    const g = project('g', 300);
    for (const direction of ['asc', 'desc'] as const) {
      expect(ids(sortProjects([f, e, g], [created(direction)], ctx))).toEqual(['f', 'e', 'g']);
      expect(ids(sortProjects([g, f, e], [created(direction)], ctx))).toEqual(['g', 'f', 'e']);
    }
    // Untagged projects tie with each other, last.
    const untagged = [project('x', 3), project('y', 2), project('z', 1)];
    for (const direction of ['asc', 'desc'] as const) {
      expect(
        ids(sortProjects([untagged[1], c, untagged[0]], [byGroup('priority', direction)], ctx))
      ).toEqual(['c', 'y', 'x']);
    }
  });

  it('composes keys in order, each breaking the ties before it', () => {
    // Priority puts c first, ties a with d, and leaves b last; created
    // oldest-first then orders the tie.
    expect(ids(sortProjects(projects, [byGroup('priority', 'asc'), created('asc')], ctx))).toEqual([
      'c',
      'd',
      'a',
      'b',
    ]);
    // The other way round, created decides everything and priority nothing.
    expect(ids(sortProjects(projects, [created('asc'), byGroup('priority', 'asc')], ctx))).toEqual([
      'd',
      'c',
      'b',
      'a',
    ]);
  });

  it('composes three keys, each with its own direction', () => {
    const activity = new Map([
      ['a', 800],
      ['c', 800],
      ['d', 900],
    ]);
    // Status, reversed: b (done), then a and c (doing), then d (idea).
    // Last updated ties a with c at 800; created oldest-first picks c.
    const sort = [byGroup('status', 'desc'), updated('desc'), created('asc')];
    expect(ids(sortProjects(projects, sort, contextOf(state, activity)))).toEqual([
      'b',
      'c',
      'a',
      'd',
    ]);
  });

  it('follows a reorder of the group’s tags', () => {
    const sort = [byGroup('priority', 'asc')];
    expect(ids(sortProjects(projects, sort, ctx))).toEqual(['c', 'a', 'd', 'b']);

    // P1 dragged ahead of P0.
    const reordered: TagState = {
      ...state,
      tags: state.tags.map((t) =>
        t.id === 'p0' ? { ...t, position: 1 } : t.id === 'p1' ? { ...t, position: 0 } : t
      ),
    };
    expect(ids(sortProjects(projects, sort, contextOf(reordered)))).toEqual(['a', 'd', 'c', 'b']);
  });

  it('sorts tags that tie on position as the catalog orders them', () => {
    // Both at position 0: the catalog falls back to creation, then id, so
    // p0 still comes first, and so does its project.
    const tied: TagState = {
      ...state,
      tags: state.tags.map((t) => (t.id === 'p1' ? { ...t, position: 0 } : t)),
    };
    expect(ids(sortProjects(projects, [byGroup('priority', 'asc')], contextOf(tied)))).toEqual([
      'c',
      'a',
      'd',
      'b',
    ]);
  });

  it('counts a tag the catalog doesn’t know as no tag', () => {
    const stray = contextOf(state);
    const projectTagIds = new Map(stray.projectTagIds).set('b', new Set(['deleted-tag']));
    expect(
      ids(sortProjects(projects, [byGroup('priority', 'asc')], { ...stray, projectTagIds }))
    ).toEqual(['c', 'a', 'd', 'b']);
  });

  describe('with the previous result', () => {
    const sort = [byGroup('priority', 'asc')];

    it('hands the previous array back when the order is unchanged', () => {
      const first = sortProjects(projects, sort, ctx);
      // A new input array and a new context, same projects in the same places.
      const again = sortProjects([...projects], sort, contextOf(tagState(ASSIGNMENTS)), first);
      expect(again).toBe(first);
    });

    it('hands the previous array back when activity moves without moving a project', () => {
      const by = [updated('desc')];
      const first = sortProjects(projects, by, contextOf(state, new Map([['c', 900]])));
      expect(ids(first)).toEqual(['c', 'a', 'b', 'd']);
      const again = sortProjects(projects, by, contextOf(state, new Map([['c', 950]])), first);
      expect(again).toBe(first);
    });

    it('returns a new array when the order changes', () => {
      const first = sortProjects(projects, sort, ctx);
      const flipped = sortProjects(projects, [byGroup('priority', 'desc')], ctx, first);
      expect(flipped).not.toBe(first);
      expect(ids(flipped)).toEqual(['a', 'd', 'c', 'b']);
      // The previous result is left as it was.
      expect(ids(first)).toEqual(['c', 'a', 'd', 'b']);
    });

    it('returns a new array when a project object was replaced in place', () => {
      const first = sortProjects(projects, sort, ctx);
      const renamed = { ...a, name: 'Renamed' };
      const again = sortProjects([renamed, b, c, d], sort, ctx, first);
      expect(again).not.toBe(first);
      expect(ids(again)).toEqual(ids(first));
      expect(again[1]).toBe(renamed);
    });

    it('returns a new array when a project came or went', () => {
      const first = sortProjects(projects, sort, ctx);
      expect(ids(sortProjects([a, b, c], sort, ctx, first))).toEqual(['c', 'a', 'b']);
      const e = project('e', 500);
      expect(ids(sortProjects([e, ...projects], sort, ctx, first))).toEqual([
        'c',
        'a',
        'd',
        'e',
        'b',
      ]);
    });
  });
});

describe('lastUpdatedAt', () => {
  it('is the latest activity, floored at the created date', () => {
    expect(lastUpdatedAt(a, new Map([['a', 900]]))).toBe(900);
    expect(lastUpdatedAt(a, new Map([['a', 50]]))).toBe(400);
    expect(lastUpdatedAt(a, new Map([['b', 900]]))).toBe(400);
    expect(lastUpdatedAt(a, null)).toBe(400);
    expect(lastUpdatedAt(a, undefined)).toBe(400);
  });
});

// ── On a board ──

describe('isInertOnBoard', () => {
  it('marks only the key for the group the board is by', () => {
    expect(isInertOnBoard(byGroup('priority', 'asc'), 'priority')).toBe(true);
    expect(isInertOnBoard(byGroup('priority', 'desc'), 'priority')).toBe(true);
    expect(isInertOnBoard(byGroup('status', 'asc'), 'priority')).toBe(false);
    expect(isInertOnBoard(created('desc'), 'priority')).toBe(false);
    expect(isInertOnBoard(updated('desc'), 'priority')).toBe(false);
  });

  it('marks nothing where there is no board', () => {
    expect(isInertOnBoard(byGroup('priority', 'asc'), null)).toBe(false);
    expect(isInertOnBoard(created('desc'), null)).toBe(false);
  });

  it('is why a board’s columns follow the next key', () => {
    // A board by Priority, sorted by Priority (either way) then oldest first.
    const group = catalog.groupsById.get('priority')!;
    const tags = catalog.tagsByGroup.get('priority')!;
    const cardsIn = (sort: ProjectSort) => {
      const layout = buildBoardColumns({
        group,
        tags,
        projects: sortProjects(projects, sort, ctx),
        groupTagIdOf: (id) =>
          [...(ctx.projectTagIds.get(id) ?? [])].find(
            (tagId) => catalog.tagsById.get(tagId)?.groupId === 'priority'
          ) ?? null,
        isVisible: () => true,
        prefs: { collapsed: new Set(), hidden: new Set() },
      });
      return layout.columns.map((column) => [column.name, ids(column.projects)]);
    };
    const oldestFirst = [
      ['No Priority', ['b']],
      ['P0', ['c']],
      ['P1', ['d', 'a']],
    ];
    expect(cardsIn([created('asc')])).toEqual(oldestFirst);
    expect(cardsIn([byGroup('priority', 'asc'), created('asc')])).toEqual(oldestFirst);
    expect(cardsIn([byGroup('priority', 'desc'), created('asc')])).toEqual(oldestFirst);
  });
});

// ── Edits ──

describe('addableSortKeys', () => {
  it('offers created, last updated, then every group in catalog order', () => {
    expect(addableSortKeys([], catalog)).toEqual([
      created('desc'),
      updated('desc'),
      byGroup('status', 'asc'),
      byGroup('priority', 'asc'),
    ]);
  });

  it('leaves out the keys in use, whichever way they point', () => {
    expect(addableSortKeys([byGroup('status', 'desc'), created('asc')], catalog)).toEqual([
      updated('desc'),
      byGroup('priority', 'asc'),
    ]);
  });

  it('offers nothing once every key is in use', () => {
    const all = [
      created('asc'),
      updated('asc'),
      byGroup('priority', 'asc'),
      byGroup('status', 'asc'),
    ];
    expect(addableSortKeys(all, catalog)).toEqual([]);
  });

  it('follows the groups’ order', () => {
    const swapped = tagState();
    swapped.groups = swapped.groups.map((g) => ({ ...g, position: g.id === 'status' ? 1 : 0 }));
    expect(addableSortKeys([created('desc'), updated('desc')], indexTagCatalog(swapped))).toEqual([
      byGroup('priority', 'asc'),
      byGroup('status', 'asc'),
    ]);
  });
});

describe('withSortKeyAdded', () => {
  it('adds the key last', () => {
    expect(withSortKeyAdded([], updated('desc'), catalog)).toEqual([updated('desc')]);
    expect(withSortKeyAdded([updated('desc')], byGroup('priority', 'asc'), catalog)).toEqual([
      updated('desc'),
      byGroup('priority', 'asc'),
    ]);
  });

  it('changes nothing for a key already in the sort, whichever way it points', () => {
    const sort = [created('asc'), byGroup('priority', 'desc')];
    expect(withSortKeyAdded(sort, created('desc'), catalog)).toBe(sort);
    expect(withSortKeyAdded(sort, byGroup('priority', 'asc'), catalog)).toBe(sort);
  });

  it('won’t add a key for a group the catalog doesn’t have', () => {
    const sort = [created('asc')];
    expect(withSortKeyAdded(sort, byGroup('gone', 'asc'), catalog)).toBe(sort);
  });

  it('prunes keys for deleted groups', () => {
    expect(withSortKeyAdded([byGroup('gone', 'asc')], created('desc'), catalog)).toEqual([
      created('desc'),
    ]);
  });
});

describe('withoutSortKey', () => {
  it('removes the key, keeping the rest in order', () => {
    const sort = [byGroup('priority', 'asc'), updated('desc'), created('asc')];
    expect(withoutSortKey(sort, 'updated', catalog)).toEqual([
      byGroup('priority', 'asc'),
      created('asc'),
    ]);
    expect(withoutSortKey(sort, 'group:priority', catalog)).toEqual([
      updated('desc'),
      created('asc'),
    ]);
    expect(withoutSortKey([created('asc')], 'created', catalog)).toEqual([]);
  });

  it('changes nothing for a key the sort doesn’t have', () => {
    const sort = [created('asc')];
    expect(withoutSortKey(sort, 'updated', catalog)).toBe(sort);
    expect(withoutSortKey(sort, 'group:status', catalog)).toBe(sort);
  });

  it('prunes keys for deleted groups', () => {
    const sort = [byGroup('gone', 'asc'), created('asc'), updated('desc')];
    expect(withoutSortKey(sort, 'created', catalog)).toEqual([updated('desc')]);
    expect(withoutSortKey(sort, 'group:gone', catalog)).toEqual([created('asc'), updated('desc')]);
  });
});

describe('withSortKeyMoved', () => {
  const sort = [byGroup('priority', 'asc'), updated('desc'), created('asc')];

  it('moves a key one place up or down', () => {
    expect(withSortKeyMoved(sort, 'updated', -1, catalog)).toEqual([
      updated('desc'),
      byGroup('priority', 'asc'),
      created('asc'),
    ]);
    expect(withSortKeyMoved(sort, 'updated', 1, catalog)).toEqual([
      byGroup('priority', 'asc'),
      created('asc'),
      updated('desc'),
    ]);
  });

  it('changes nothing at either end, or for a key the sort doesn’t have', () => {
    expect(withSortKeyMoved(sort, 'group:priority', -1, catalog)).toBe(sort);
    expect(withSortKeyMoved(sort, 'created', 1, catalog)).toBe(sort);
    expect(withSortKeyMoved(sort, 'group:status', 1, catalog)).toBe(sort);
    expect(sort).toEqual([byGroup('priority', 'asc'), updated('desc'), created('asc')]);
  });

  it('steps past a deleted group’s key, pruning it', () => {
    const stale = [created('asc'), byGroup('gone', 'asc'), updated('desc')];
    // One step, not two: the dead key between them doesn't count.
    expect(withSortKeyMoved(stale, 'updated', -1, catalog)).toEqual([
      updated('desc'),
      created('asc'),
    ]);
    // Already last of the keys that count.
    expect(withSortKeyMoved(stale, 'updated', 1, catalog)).toEqual([
      created('asc'),
      updated('desc'),
    ]);
  });
});

describe('withSortKeyDirection', () => {
  const sort = [byGroup('priority', 'asc'), created('desc')];

  it('points the key the other way, in place', () => {
    expect(withSortKeyDirection(sort, 'group:priority', 'desc', catalog)).toEqual([
      byGroup('priority', 'desc'),
      created('desc'),
    ]);
    expect(withSortKeyDirection(sort, 'created', 'asc', catalog)).toEqual([
      byGroup('priority', 'asc'),
      created('asc'),
    ]);
    // The sort it was given is left as it was.
    expect(sort).toEqual([byGroup('priority', 'asc'), created('desc')]);
  });

  it('changes nothing for the direction it already has, or a key the sort doesn’t have', () => {
    expect(withSortKeyDirection(sort, 'created', 'desc', catalog)).toBe(sort);
    expect(withSortKeyDirection(sort, 'updated', 'asc', catalog)).toBe(sort);
  });

  it('prunes keys for deleted groups', () => {
    expect(
      withSortKeyDirection([byGroup('gone', 'asc'), created('desc')], 'created', 'asc', catalog)
    ).toEqual([created('asc')]);
  });
});

// ── Describing ──

describe('sortKeyLabel', () => {
  it('names the dates, and a group key by its group', () => {
    expect(sortKeyLabel(created('desc'), catalog)).toBe('Created');
    expect(sortKeyLabel(updated('asc'), catalog)).toBe('Last updated');
    expect(sortKeyLabel(byGroup('priority', 'asc'), catalog)).toBe('Priority');
    expect(sortKeyLabel(byGroup('status', 'desc'), catalog)).toBe('Status');
  });

  it('has no name for a group the catalog doesn’t have', () => {
    expect(sortKeyLabel(byGroup('gone', 'asc'), catalog)).toBe('');
  });
});

describe('sortSummary', () => {
  it('reads "Newest" with no keys', () => {
    expect(sortSummary([], catalog)).toBe('Newest');
    expect(DEFAULT_SORT_SUMMARY).toBe('Newest');
  });

  it('lists the keys’ names in order', () => {
    expect(sortSummary([byGroup('priority', 'asc')], catalog)).toBe('Priority');
    expect(sortSummary([byGroup('priority', 'desc'), updated('desc')], catalog)).toBe(
      'Priority, Last updated'
    );
    expect(sortSummary([created('asc'), byGroup('status', 'asc')], catalog)).toBe(
      'Created, Status'
    );
  });

  it('leaves out keys for deleted groups', () => {
    expect(sortSummary([byGroup('gone', 'asc'), updated('desc')], catalog)).toBe('Last updated');
    expect(sortSummary([byGroup('gone', 'asc')], catalog)).toBe('Newest');
  });
});
