import { describe, expect, it } from 'vitest';
import { indexTagCatalog } from '../tags/tagState';
import { tagState } from '../tags/testStubs/tagFixtures';
import {
  NO_TAG_COLUMN_KEY,
  columnPrefsFor,
  normalizeBoardColumns,
  normalizeBoardGroupId,
  normalizeViewMode,
  pruneBoardColumns,
  resolveBoardGroup,
  withColumnFlag,
} from './boardPrefs';

// Status (icon group: idea, doing, done), Priority (p0, p1), ungrouped q4, infra.
const catalog = indexTagCatalog(tagState());

describe('normalizeViewMode', () => {
  it('reads board, and anything else as the grid', () => {
    expect(normalizeViewMode('board')).toBe('board');
    expect(normalizeViewMode('grid')).toBe('grid');
    expect(normalizeViewMode(undefined)).toBe('grid');
    expect(normalizeViewMode('kanban')).toBe('grid');
    expect(normalizeViewMode(1)).toBe('grid');
  });
});

describe('normalizeBoardGroupId', () => {
  it('keeps a non-empty string, else null', () => {
    expect(normalizeBoardGroupId('priority')).toBe('priority');
    expect(normalizeBoardGroupId('')).toBeNull();
    expect(normalizeBoardGroupId(null)).toBeNull();
    expect(normalizeBoardGroupId(42)).toBeNull();
  });
});

describe('normalizeBoardColumns', () => {
  it('keeps well-formed boards, dropping malformed entries and duplicate keys', () => {
    expect(
      normalizeBoardColumns({
        status: { collapsed: ['done', 'done', 3], hidden: [NO_TAG_COLUMN_KEY] },
        priority: { hidden: 'p0' },
        broken: 'nope',
        list: [],
      })
    ).toEqual({
      status: { collapsed: ['done'], hidden: [NO_TAG_COLUMN_KEY] },
      priority: { collapsed: [], hidden: [] },
    });
  });

  it('reads anything but an object as no column state', () => {
    expect(normalizeBoardColumns(undefined)).toEqual({});
    expect(normalizeBoardColumns(['status'])).toEqual({});
    expect(normalizeBoardColumns('status')).toEqual({});
  });

  it('keeps ids the catalog doesn’t know, for the next write to prune', () => {
    expect(normalizeBoardColumns({ gone: { collapsed: ['x'], hidden: [] } })).toEqual({
      gone: { collapsed: ['x'], hidden: [] },
    });
  });
});

describe('resolveBoardGroup', () => {
  it('uses the chosen group while it exists', () => {
    expect(resolveBoardGroup('priority', catalog)?.id).toBe('priority');
  });

  it('falls back to the icon group, then the first group', () => {
    expect(resolveBoardGroup('deleted', catalog)?.id).toBe('status');
    expect(resolveBoardGroup(null, catalog)?.id).toBe('status');

    const noIcon = tagState();
    noIcon.groups = noIcon.groups.map((group) => ({ ...group, isIconGroup: false }));
    noIcon.groups[0].position = 5;
    expect(resolveBoardGroup('deleted', indexTagCatalog(noIcon))?.id).toBe('priority');
  });

  it('is null without groups', () => {
    const empty = indexTagCatalog({ groups: [], tags: [], assignments: [] });
    expect(resolveBoardGroup('status', empty)).toBeNull();
    expect(resolveBoardGroup(null, empty)).toBeNull();
  });
});

describe('columnPrefsFor', () => {
  it('ignores keys that are no longer the board’s columns', () => {
    const prefs = columnPrefsFor(
      {
        status: {
          // p0 is Priority's, q4 ungrouped, gone deleted.
          collapsed: ['done', 'p0', 'gone', NO_TAG_COLUMN_KEY],
          hidden: ['idea', 'q4'],
        },
      },
      'status',
      catalog
    );
    expect([...prefs.collapsed]).toEqual(['done', NO_TAG_COLUMN_KEY]);
    expect([...prefs.hidden]).toEqual(['idea']);
  });

  it('is empty for a board with no state', () => {
    const prefs = columnPrefsFor({}, 'priority', catalog);
    expect(prefs.collapsed.size).toBe(0);
    expect(prefs.hidden.size).toBe(0);
  });
});

describe('pruneBoardColumns', () => {
  it('drops deleted groups, stray keys and boards left empty, keeping stored order', () => {
    expect(
      pruneBoardColumns(
        {
          status: { collapsed: ['done', 'p0', 'idea'], hidden: ['gone'] },
          priority: { collapsed: ['idea'], hidden: [] },
          deleted: { collapsed: ['x'], hidden: ['y'] },
        },
        catalog
      )
    ).toEqual({ status: { collapsed: ['done', 'idea'], hidden: [] } });
  });
});

describe('withColumnFlag', () => {
  it('turns a flag on and off for one column, pruning the rest', () => {
    let columns = withColumnFlag(
      { deleted: { collapsed: ['x'], hidden: [] } },
      'status',
      'done',
      'collapsed',
      true,
      catalog
    );
    expect(columns).toEqual({ status: { collapsed: ['done'], hidden: [] } });

    columns = withColumnFlag(columns, 'status', NO_TAG_COLUMN_KEY, 'hidden', true, catalog);
    columns = withColumnFlag(columns, 'status', NO_TAG_COLUMN_KEY, 'hidden', true, catalog);
    expect(columns).toEqual({
      status: { collapsed: ['done'], hidden: [NO_TAG_COLUMN_KEY] },
    });

    columns = withColumnFlag(columns, 'status', 'done', 'collapsed', false, catalog);
    columns = withColumnFlag(columns, 'status', NO_TAG_COLUMN_KEY, 'hidden', false, catalog);
    expect(columns).toEqual({});
  });

  it('won’t record a key that isn’t one of the board’s columns', () => {
    expect(withColumnFlag({}, 'status', 'p0', 'hidden', true, catalog)).toEqual({});
  });
});
