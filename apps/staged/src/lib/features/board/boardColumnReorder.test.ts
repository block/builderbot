import { describe, expect, it } from 'vitest';
import type { TagState } from '../../types';
import { withTagMoved } from '../tags/tagCatalogPatches';
import { resolveTagDrop } from '../tags/tagReorder';
import { indexTagCatalog } from '../tags/tagState';
import { group, scopeIds, tag } from '../tags/testStubs/tagFixtures';
import { columnDropTarget, columnStepTarget, type ColumnDrop } from './boardColumnReorder';
import { NO_TAG_COLUMN_KEY } from './boardPrefs';

/** A Stage board with columns a to e, plus an Other group holding x. */
function boardState(order: readonly string[] = ['a', 'b', 'c', 'd', 'e']): TagState {
  return {
    groups: [group('stage', 0), group('other', 1)],
    tags: [...order.map((id, position) => tag(id, 'stage', position)), tag('x', 'other', 0)],
    assignments: [],
  };
}

const NONE: ReadonlySet<string> = new Set();

function hide(...ids: string[]): ReadonlySet<string> {
  return new Set(ids);
}

function before(columnKey: string): ColumnDrop {
  return { columnKey, place: 'before' };
}

function after(columnKey: string): ColumnDrop {
  return { columnKey, place: 'after' };
}

function dropTarget(state: TagState, hidden: ReadonlySet<string>, tagId: string, drop: ColumnDrop) {
  return columnDropTarget(indexTagCatalog(state), 'stage', hidden, tagId, drop);
}

/**
 * The Stage tags in order after applying `target` to `tagId` the way the
 * board does: through the tag drop resolver, then the move it resolves to.
 * Null when there is no target, or it resolves to no move.
 */
function applied(
  state: TagState,
  tagId: string,
  target: ReturnType<typeof columnDropTarget>
): string[] | null {
  const move = target && resolveTagDrop(indexTagCatalog(state), tagId, target);
  return move ? scopeIds(withTagMoved(state, move), 'stage') : null;
}

/** The Stage tags in order after dropping `tagId`'s column on `drop`, or null for no move. */
function dropped(
  state: TagState,
  hidden: ReadonlySet<string>,
  tagId: string,
  drop: ColumnDrop
): string[] | null {
  return applied(state, tagId, dropTarget(state, hidden, tagId, drop));
}

/** The Stage tags in order after stepping `tagId`'s column, or null for no move. */
function stepped(
  state: TagState,
  hidden: ReadonlySet<string>,
  tagId: string,
  step: -1 | 1
): string[] | null {
  const target = columnStepTarget(indexTagCatalog(state), 'stage', hidden, tagId, step);
  return applied(state, tagId, target);
}

describe('columnDropTarget', () => {
  it('names the column the drop was beside as the tag drop anchor', () => {
    expect(dropTarget(boardState(), NONE, 'a', after('c'))).toEqual({
      groupId: 'stage',
      anchorTagId: 'c',
      place: 'after',
    });
    expect(dropTarget(boardState(), NONE, 'e', before('b'))).toEqual({
      groupId: 'stage',
      anchorTagId: 'b',
      place: 'before',
    });
  });

  it('moves a column right, after its anchor', () => {
    expect(dropped(boardState(), NONE, 'a', after('c'))).toEqual(['b', 'c', 'a', 'd', 'e']);
    expect(dropped(boardState(), NONE, 'a', before('c'))).toEqual(['b', 'a', 'c', 'd', 'e']);
    expect(dropped(boardState(), NONE, 'a', after('e'))).toEqual(['b', 'c', 'd', 'e', 'a']);
  });

  it('moves a column left, before its anchor', () => {
    expect(dropped(boardState(), NONE, 'e', before('b'))).toEqual(['a', 'e', 'b', 'c', 'd']);
    expect(dropped(boardState(), NONE, 'e', after('b'))).toEqual(['a', 'b', 'e', 'c', 'd']);
    expect(dropped(boardState(), NONE, 'e', before('a'))).toEqual(['e', 'a', 'b', 'c', 'd']);
  });

  it('is a no-op when dropped on itself', () => {
    expect(dropTarget(boardState(), NONE, 'c', before('c'))).toBeNull();
    expect(dropTarget(boardState(), NONE, 'c', after('c'))).toBeNull();
  });

  it('is a no-op when the column would land where it is', () => {
    // Just right of the column on its left, or just left of the one on its right.
    expect(dropTarget(boardState(), NONE, 'c', after('b'))).toBeNull();
    expect(dropTarget(boardState(), NONE, 'c', before('d'))).toBeNull();
  });
});

describe('columnDropTarget: beside "No ‹group›"', () => {
  it('puts the column first, on either side of it', () => {
    for (const drop of [before(NO_TAG_COLUMN_KEY), after(NO_TAG_COLUMN_KEY)]) {
      expect(dropTarget(boardState(), NONE, 'd', drop)).toEqual({
        groupId: 'stage',
        anchorTagId: 'a',
        place: 'before',
      });
      expect(dropped(boardState(), NONE, 'd', drop)).toEqual(['d', 'a', 'b', 'c', 'e']);
    }
  });

  it('is a no-op for the column that is already first', () => {
    expect(dropTarget(boardState(), NONE, 'a', after(NO_TAG_COLUMN_KEY))).toBeNull();
    expect(dropTarget(boardState(), NONE, 'a', before(NO_TAG_COLUMN_KEY))).toBeNull();
  });

  it('puts the column first of those showing, leaving hidden ones ahead of it', () => {
    expect(dropped(boardState(), hide('a', 'b'), 'e', after(NO_TAG_COLUMN_KEY))).toEqual([
      'a',
      'b',
      'e',
      'c',
      'd',
    ]);
  });

  it('is a no-op for the first showing column, though hidden ones come before it', () => {
    expect(dropTarget(boardState(), hide('a', 'b'), 'c', after(NO_TAG_COLUMN_KEY))).toBeNull();
  });

  it('is a no-op when the column is the only one showing', () => {
    expect(
      dropTarget(boardState(), hide('a', 'b', 'd', 'e'), 'c', after(NO_TAG_COLUMN_KEY))
    ).toBeNull();
  });

  it('still works with "No ‹group›" itself hidden from the board', () => {
    // Its key can be in the hidden set; it is never one of the tag columns.
    expect(dropped(boardState(), hide(NO_TAG_COLUMN_KEY), 'c', before(NO_TAG_COLUMN_KEY))).toEqual([
      'c',
      'a',
      'b',
      'd',
      'e',
    ]);
  });
});

describe('columnDropTarget: hidden columns', () => {
  it('lands next to the showing anchor, across hidden columns between source and target', () => {
    // Showing: a, c, e. The hidden b and d keep their order among the rest.
    const hidden = hide('b', 'd');
    expect(dropped(boardState(), hidden, 'a', after('e'))).toEqual(['b', 'c', 'd', 'e', 'a']);
    expect(dropped(boardState(), hidden, 'a', after('c'))).toEqual(['b', 'c', 'a', 'd', 'e']);
    expect(dropped(boardState(), hidden, 'a', before('e'))).toEqual(['b', 'c', 'd', 'a', 'e']);
    expect(dropped(boardState(), hidden, 'e', before('a'))).toEqual(['e', 'a', 'b', 'c', 'd']);
    expect(dropped(boardState(), hidden, 'e', before('c'))).toEqual(['a', 'b', 'e', 'c', 'd']);
    expect(dropped(boardState(), hidden, 'e', after('a'))).toEqual(['a', 'e', 'b', 'c', 'd']);
  });

  it('leaves every column but the dragged one in the order it was in', () => {
    const hidden = hide('b', 'd');
    for (const [tagId, drop] of [
      ['a', after('e')],
      ['a', after('c')],
      ['e', before('a')],
      ['e', before('c')],
      ['c', before('a')],
      ['c', after('e')],
      ['e', after(NO_TAG_COLUMN_KEY)],
    ] as const) {
      const order = dropped(boardState(), hidden, tagId, drop);
      expect(order?.filter((id) => id !== tagId)).toEqual(
        ['a', 'b', 'c', 'd', 'e'].filter((id) => id !== tagId)
      );
    }
  });

  it('is a no-op when the showing columns would keep their order', () => {
    // Showing: a, c, e. "Before c" is where a already is on the board, and
    // moving it past the hidden b would be a write nobody could see.
    const hidden = hide('b', 'd');
    expect(dropTarget(boardState(), hidden, 'a', before('c'))).toBeNull();
    expect(dropTarget(boardState(), hidden, 'c', after('a'))).toBeNull();
    expect(dropTarget(boardState(), hidden, 'c', before('e'))).toBeNull();
    expect(dropTarget(boardState(), hidden, 'e', after('c'))).toBeNull();
  });

  it('is a no-op beside a column that is hidden, or for a dragged column that is', () => {
    expect(dropTarget(boardState(), hide('c'), 'a', after('c'))).toBeNull();
    expect(dropTarget(boardState(), hide('a'), 'a', after('c'))).toBeNull();
  });
});

describe('columnDropTarget: catalog changed mid-drag', () => {
  it('is a no-op when the dragged column was deleted', () => {
    const state = boardState(['a', 'b', 'c']);
    expect(dropTarget(state, NONE, 'e', before('a'))).toBeNull();
    expect(dropTarget(state, NONE, 'e', after(NO_TAG_COLUMN_KEY))).toBeNull();
  });

  it('is a no-op when the anchor column was deleted', () => {
    expect(dropTarget(boardState(['a', 'b', 'c']), NONE, 'a', after('e'))).toBeNull();
  });

  it('is a no-op when either column moved to another group', () => {
    // x is a tag, but in Other: it was a Stage column when the drag began.
    expect(dropTarget(boardState(), NONE, 'a', after('x'))).toBeNull();
    expect(dropTarget(boardState(), NONE, 'x', after('c'))).toBeNull();
    expect(dropTarget(boardState(), NONE, 'x', after(NO_TAG_COLUMN_KEY))).toBeNull();
  });

  it('is a no-op when the group was deleted', () => {
    const state = boardState();
    const gone: TagState = {
      ...state,
      groups: state.groups.filter((g) => g.id !== 'stage'),
      tags: state.tags.filter((t) => t.groupId !== 'stage'),
    };
    expect(dropTarget(gone, NONE, 'a', after('c'))).toBeNull();
    expect(dropTarget(gone, NONE, 'c', after(NO_TAG_COLUMN_KEY))).toBeNull();
  });

  it('resolves against the columns at drop time', () => {
    // Another window moved e first and deleted b after the drag began.
    const state = boardState(['e', 'a', 'c', 'd']);
    expect(dropped(state, NONE, 'a', after('c'))).toEqual(['e', 'c', 'a', 'd']);
    // "Before a" was a move for e when the drag began; it is where e is now.
    expect(dropTarget(state, NONE, 'e', before('a'))).toBeNull();
    // "First" is before e now, not before a.
    expect(dropTarget(state, NONE, 'd', after(NO_TAG_COLUMN_KEY))).toMatchObject({
      anchorTagId: 'e',
      place: 'before',
    });
  });

  it('resolves against the hidden columns at drop time', () => {
    // c was hidden after the drag began, so a and d are neighbours now.
    expect(dropTarget(boardState(), hide('b', 'c'), 'a', before('d'))).toBeNull();
    expect(dropped(boardState(), hide('b', 'c'), 'a', after('d'))).toEqual([
      'b',
      'c',
      'd',
      'a',
      'e',
    ]);
  });
});

describe('columnStepTarget', () => {
  it('targets the column on the left, or on the right', () => {
    const catalog = indexTagCatalog(boardState());
    expect(columnStepTarget(catalog, 'stage', NONE, 'c', -1)).toEqual({
      groupId: 'stage',
      anchorTagId: 'b',
      place: 'before',
    });
    expect(columnStepTarget(catalog, 'stage', NONE, 'c', 1)).toEqual({
      groupId: 'stage',
      anchorTagId: 'd',
      place: 'after',
    });
  });

  it('moves a column one step', () => {
    expect(stepped(boardState(), NONE, 'c', -1)).toEqual(['a', 'c', 'b', 'd', 'e']);
    expect(stepped(boardState(), NONE, 'c', 1)).toEqual(['a', 'b', 'd', 'c', 'e']);
  });

  it('moves past the nearest showing column, and the hidden ones between', () => {
    // Showing: a, d, e.
    const hidden = hide('b', 'c');
    expect(stepped(boardState(), hidden, 'a', 1)).toEqual(['b', 'c', 'd', 'a', 'e']);
    expect(stepped(boardState(), hidden, 'd', -1)).toEqual(['d', 'a', 'b', 'c', 'e']);
    expect(stepped(boardState(), hidden, 'd', 1)).toEqual(['a', 'b', 'c', 'e', 'd']);
  });

  it('is null at either end of the showing columns', () => {
    expect(stepped(boardState(), NONE, 'a', -1)).toBeNull();
    expect(stepped(boardState(), NONE, 'e', 1)).toBeNull();
    // Hidden columns beyond the end are not somewhere to step to.
    expect(stepped(boardState(), hide('a', 'e'), 'b', -1)).toBeNull();
    expect(stepped(boardState(), hide('a', 'e'), 'd', 1)).toBeNull();
    expect(stepped(boardState(), hide('a', 'b', 'd', 'e'), 'c', -1)).toBeNull();
    expect(stepped(boardState(), hide('a', 'b', 'd', 'e'), 'c', 1)).toBeNull();
  });

  it('is null for a column that is hidden, gone, in another group, or "No ‹group›"', () => {
    const catalog = indexTagCatalog(boardState());
    expect(columnStepTarget(catalog, 'stage', hide('c'), 'c', 1)).toBeNull();
    expect(columnStepTarget(catalog, 'stage', NONE, 'gone', 1)).toBeNull();
    expect(columnStepTarget(catalog, 'stage', NONE, 'x', 1)).toBeNull();
    expect(columnStepTarget(catalog, 'stage', NONE, NO_TAG_COLUMN_KEY, 1)).toBeNull();
    expect(columnStepTarget(catalog, 'gone', NONE, 'c', 1)).toBeNull();
  });

  it('steps against the catalog as it is now', () => {
    // b was deleted, so c's left neighbour is a.
    expect(stepped(boardState(['a', 'c', 'd', 'e']), NONE, 'c', -1)).toEqual(['c', 'a', 'd', 'e']);
  });
});
