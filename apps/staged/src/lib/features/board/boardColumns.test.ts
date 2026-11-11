import { describe, expect, it } from 'vitest';
import type { Project } from '../../types';
import { indexTagCatalog } from '../tags/tagState';
import { tagState } from '../tags/testStubs/tagFixtures';
import { buildBoardColumns, hiddenBoardColumns, noTagColumnName } from './boardColumns';
import { NO_TAG_COLUMN_KEY, type ResolvedColumnPrefs } from './boardPrefs';

const catalog = indexTagCatalog(tagState());
const status = catalog.groupsById.get('status')!;
const statusTags = catalog.tagsByGroup.get('status')!;

function project(id: string): Project {
  return {
    id,
    name: id,
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
  };
}

const NO_PREFS: ResolvedColumnPrefs = { collapsed: new Set(), hidden: new Set() };

function build(
  tagOf: Record<string, string | null>,
  {
    prefs = NO_PREFS,
    visible = () => true,
  }: { prefs?: ResolvedColumnPrefs; visible?: (id: string) => boolean } = {}
) {
  return buildBoardColumns({
    group: status,
    tags: statusTags,
    projects: Object.keys(tagOf).map(project),
    groupTagIdOf: (id) => tagOf[id] ?? null,
    isVisible: visible,
    prefs,
  });
}

function cards(layout: ReturnType<typeof build>): Record<string, string[]> {
  return Object.fromEntries(
    layout.columns.map((column) => [column.key, column.projects.map((p) => p.id)])
  );
}

describe('buildBoardColumns', () => {
  it('puts "No ‹group›" first, then a column per tag in order, cards in project order', () => {
    const layout = build({ newest: 'done', mid: null, older: 'idea', oldest: 'done' });

    expect(layout.columns.map((column) => column.name)).toEqual([
      'No Status',
      'Idea',
      'Doing',
      'Done',
    ]);
    expect(cards(layout)).toEqual({
      [NO_TAG_COLUMN_KEY]: ['mid'],
      idea: ['older'],
      doing: [],
      done: ['newest', 'oldest'],
    });
    expect(layout.columns.map((column) => column.count)).toEqual([1, 1, 0, 2]);
    expect(layout.columns[0].tag).toBeNull();
    expect(layout.columns[1].tag?.id).toBe('idea');
    expect(layout.hidden).toEqual([]);
  });

  it('counts only the cards a search leaves showing, but keeps every card', () => {
    const layout = build({ a: 'done', b: 'done', c: null }, { visible: (id) => id === 'b' });

    expect(cards(layout).done).toEqual(['a', 'b']);
    expect(layout.columns.map((column) => column.count)).toEqual([0, 0, 0, 1]);
  });

  it('files a project whose tag is no longer one of the columns under "No ‹group›"', () => {
    const layout = build({ a: 'gone', b: 'p0' });
    expect(cards(layout)[NO_TAG_COLUMN_KEY]).toEqual(['a', 'b']);
  });

  it('leaves hidden columns and their cards out, listing them in column order', () => {
    const layout = build(
      { a: 'done', b: null, c: 'doing' },
      { prefs: { collapsed: new Set(), hidden: new Set(['done', NO_TAG_COLUMN_KEY]) } }
    );

    expect(layout.columns.map((column) => column.key)).toEqual(['idea', 'doing']);
    expect(cards(layout)).toEqual({ idea: [], doing: ['c'] });
    expect(layout.hidden.map((column) => [column.key, column.name])).toEqual([
      [NO_TAG_COLUMN_KEY, 'No Status'],
      ['done', 'Done'],
    ]);
    // a (Done) and b (no status) are behind the hidden columns.
    expect(layout.hiddenCount).toBe(2);
  });

  it('counts only the showing cards behind hidden columns', () => {
    const hidden = { collapsed: new Set<string>(), hidden: new Set(['done']) };

    const searched = build(
      { a: 'done', b: 'done', c: 'doing' },
      { prefs: hidden, visible: (id) => id === 'a' }
    );
    expect(searched.hiddenCount).toBe(1);
    expect(searched.columns.every((column) => column.count === 0)).toBe(true);

    expect(build({ c: 'doing' }, { prefs: hidden }).hiddenCount).toBe(0);
    expect(build({}, { prefs: NO_PREFS }).hiddenCount).toBe(0);
  });

  it('marks collapsed columns, which keep their cards and count', () => {
    const layout = build(
      { a: 'done' },
      { prefs: { collapsed: new Set(['done']), hidden: new Set() } }
    );
    const done = layout.columns.find((column) => column.key === 'done')!;

    expect(done.collapsed).toBe(true);
    expect(done.count).toBe(1);
    expect(layout.columns.filter((column) => column.collapsed)).toHaveLength(1);
  });

  it('shows just "No ‹group›" for a group with no tags', () => {
    const empty = { ...status, id: 'empty', name: 'Effort' };
    const layout = buildBoardColumns({
      group: empty,
      tags: [],
      projects: [project('a')],
      groupTagIdOf: () => null,
      isVisible: () => true,
      prefs: NO_PREFS,
    });

    expect(layout.columns.map((column) => [column.name, column.count])).toEqual([['No Effort', 1]]);
  });
});

describe('hiddenBoardColumns', () => {
  it('lists the hidden columns in column order', () => {
    expect(
      hiddenBoardColumns(status, statusTags, new Set(['done', 'idea'])).map((c) => c.key)
    ).toEqual(['idea', 'done']);
  });
});

describe('noTagColumnName', () => {
  it('names the column after the group', () => {
    expect(noTagColumnName({ name: 'Priority' })).toBe('No Priority');
  });
});
