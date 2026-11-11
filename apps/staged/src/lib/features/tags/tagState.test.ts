import { describe, expect, it } from 'vitest';
import type { Tag, TagGroup, TagState } from '../../types';
import {
  applyIntents,
  chipTags,
  currentIntent,
  effectiveProjectTagIds,
  groupTag,
  iconGroupFirst,
  indexAssignments,
  indexTagCatalog,
  intentSlot,
  limitChips,
  MAX_TAG_NAME_CHARS,
  numberedName,
  pruneConfirmedIntents,
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

describe('iconGroupFirst', () => {
  const groups: TagGroup[] = ['a', 'b', 'c'].map((id, position) => ({
    id,
    name: id.toUpperCase(),
    position,
    isIconGroup: id === 'b',
    showInSidebar: false,
    createdAt: 0,
    updatedAt: 0,
  }));

  it('puts the icon group first and keeps the rest in position order', () => {
    expect(iconGroupFirst(groups, groups[1]).map((g) => g.id)).toEqual(['b', 'a', 'c']);
  });

  it('keeps position order when no group drives the icon', () => {
    expect(iconGroupFirst(groups, null).map((g) => g.id)).toEqual(['a', 'b', 'c']);
    expect(iconGroupFirst([], null)).toEqual([]);
  });
});
