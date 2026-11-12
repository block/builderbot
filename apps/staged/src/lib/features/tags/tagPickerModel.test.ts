import { describe, expect, it } from 'vitest';
import { buildTagPickerModel, moveActiveIndex, type TagPickerRow } from './tagPickerModel';
import { indexTagCatalog } from './tagState';
import { tagState } from './testStubs/tagFixtures';

// Status (idea, doing, done) and Priority (p0, p1), plus ungrouped q4 and infra.
const catalog = indexTagCatalog(tagState());

function keys(rows: readonly TagPickerRow[]): string[] {
  return rows.map((row) => row.key);
}

describe('buildTagPickerModel', () => {
  it('lists every group in order, then the ungrouped tags, then Manage', () => {
    const model = buildTagPickerModel(catalog, new Set(), '');

    expect(model.sections.map((s) => [s.groupId, s.label])).toEqual([
      ['status', 'Status'],
      ['priority', 'Priority'],
      [null, 'Tags'],
    ]);
    expect(keys(model.rows)).toEqual([
      'tag:idea',
      'tag:doing',
      'tag:done',
      'tag:p0',
      'tag:p1',
      'tag:q4',
      'tag:infra',
      'manage',
    ]);
    expect(model.create).toBeNull();
    expect(model.defaultActive).toBe(0);
  });

  it('makes grouped tags radios and ungrouped tags checkboxes, checked by assignment', () => {
    const model = buildTagPickerModel(catalog, new Set(['doing', 'infra']), '');
    const rows = model.sections.flatMap((s) => s.rows);
    const byId = new Map(rows.map((row) => [row.tag.id, row]));

    expect(byId.get('doing')).toMatchObject({ mode: 'radio', checked: true });
    expect(byId.get('idea')).toMatchObject({ mode: 'radio', checked: false });
    expect(byId.get('infra')).toMatchObject({ mode: 'checkbox', checked: true });
    expect(byId.get('q4')).toMatchObject({ mode: 'checkbox', checked: false });
  });

  it('filters by name ignoring case, dropping empty sections', () => {
    const model = buildTagPickerModel(catalog, new Set(), '  DO ');

    expect(model.sections.map((s) => s.groupId)).toEqual(['status']);
    expect(keys(model.rows)).toEqual(['tag:doing', 'tag:done', 'create', 'manage']);
  });

  it('keeps a whole group when its name matches', () => {
    const model = buildTagPickerModel(catalog, new Set(), 'prio');

    expect(keys(model.sections.flatMap((s) => s.rows))).toEqual(['tag:p0', 'tag:p1']);
  });

  it('offers to create the query when no tag has that name, highlighted for Enter', () => {
    const model = buildTagPickerModel(catalog, new Set(), ' q4-launch ');

    expect(model.create).toEqual({
      kind: 'create',
      key: 'create',
      name: 'q4-launch',
      tooLong: false,
    });
    // Nothing contains "q4-launch", so the create row is the only choice.
    expect(keys(model.rows)).toEqual(['create', 'manage']);
    expect(model.rows[model.defaultActive].key).toBe('create');
  });

  it('highlights an exact match instead of offering to create it', () => {
    const model = buildTagPickerModel(catalog, new Set(), 'Q4');

    expect(model.create).toBeNull();
    expect(model.rows[model.defaultActive].key).toBe('tag:q4');
  });

  it('highlights the first partial match over the create row', () => {
    const model = buildTagPickerModel(catalog, new Set(), 'p');

    // "p" matches P0 and P1 (and names Priority), but no tag is named "p":
    // the create row is offered, but Enter picks the existing P0.
    expect(keys(model.rows)).toEqual(['tag:p0', 'tag:p1', 'create', 'manage']);
    expect(model.rows[model.defaultActive].key).toBe('tag:p0');
  });

  it('highlights a prefix match rather than creating a near-duplicate', () => {
    const model = buildTagPickerModel(catalog, new Set(), 'inf');

    expect(keys(model.rows)).toEqual(['tag:infra', 'create', 'manage']);
    expect(model.create?.name).toBe('inf');
    expect(model.rows[model.defaultActive].key).toBe('tag:infra');
  });

  it('highlights the first of a group whose name matches', () => {
    const model = buildTagPickerModel(catalog, new Set(), 'prio');

    expect(keys(model.rows)).toEqual(['tag:p0', 'tag:p1', 'create', 'manage']);
    expect(model.rows[model.defaultActive].key).toBe('tag:p0');
  });

  it('marks a name longer than the backend accepts', () => {
    const model = buildTagPickerModel(catalog, new Set(), 'x'.repeat(65));

    expect(model.create?.tooLong).toBe(true);
  });

  it('shows only Manage when there are no tags and no query', () => {
    const empty = indexTagCatalog({ groups: [], tags: [], assignments: [] });
    const model = buildTagPickerModel(empty, new Set(), '');

    expect(model.sections).toEqual([]);
    expect(keys(model.rows)).toEqual(['manage']);
  });
});

describe('moveActiveIndex', () => {
  it('moves and wraps at both ends', () => {
    expect(moveActiveIndex(0, 1, 3)).toBe(1);
    expect(moveActiveIndex(2, 1, 3)).toBe(0);
    expect(moveActiveIndex(0, -1, 3)).toBe(2);
  });

  it('stays at 0 with no rows', () => {
    expect(moveActiveIndex(0, 1, 0)).toBe(0);
  });
});
