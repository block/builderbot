import { describe, expect, it } from 'vitest';
import { TAG_GROUP_TEMPLATES, freeGroupName, type TagGroupTemplateId } from './tagTemplates';
import { group } from './testStubs/tagFixtures';

function template(id: TagGroupTemplateId) {
  return TAG_GROUP_TEMPLATES.find((t) => t.id === id)!;
}

describe('TAG_GROUP_TEMPLATES', () => {
  it('offers Status, Priority and Blank, in that order', () => {
    expect(TAG_GROUP_TEMPLATES.map((t) => t.label)).toEqual(['Status', 'Priority', 'Blank']);
  });

  it('recreates the seven project statuses Staged shipped with', () => {
    expect(template('status').tags).toEqual([
      { name: 'Idea', icon: 'lightbulb', color: 'amber' },
      { name: 'In progress', icon: 'circle-dot', color: 'blue' },
      { name: 'Polishing', icon: 'paintbrush', color: 'cyan' },
      { name: 'Needs review', icon: 'eye', color: 'purple' },
      { name: 'Done', icon: 'circle-check', color: 'green' },
      { name: 'On hold', icon: 'circle-pause', color: 'gray' },
      { name: 'Blocked', icon: 'octagon-minus', color: 'red' },
    ]);
  });

  it('has P0 to P3 for Priority', () => {
    expect(template('priority').tags).toEqual([
      { name: 'P0', icon: 'chevrons-up', color: 'red' },
      { name: 'P1', icon: 'chevron-up', color: 'amber' },
      { name: 'P2', icon: 'minus', color: 'blue' },
      { name: 'P3', icon: 'chevron-down', color: 'gray' },
    ]);
  });

  it('starts Blank with no tags', () => {
    expect(template('blank').tags).toEqual([]);
  });

  it('carries no ids, so the backend assigns fresh ones', () => {
    for (const t of TAG_GROUP_TEMPLATES) {
      for (const tag of t.tags) expect(tag).not.toHaveProperty('id');
    }
  });

  it('uses names unique within each template, as one group requires', () => {
    for (const t of TAG_GROUP_TEMPLATES) {
      const keys = t.tags.map((tag) => tag.name.toLowerCase());
      expect(new Set(keys).size).toBe(keys.length);
    }
  });
});

describe('freeGroupName', () => {
  it('uses the template name when no group has it', () => {
    expect(freeGroupName(template('priority'), [group('s', 0, { name: 'Status' })])).toBe(
      'Priority'
    );
  });

  it('numbers it when a group has it, ignoring case', () => {
    expect(freeGroupName(template('status'), [group('s', 0, { name: 'status' })])).toBe('Status 2');
    expect(
      freeGroupName(template('status'), [
        group('s', 0, { name: 'Status' }),
        group('s2', 1, { name: 'Status 2' }),
      ])
    ).toBe('Status 3');
  });

  it('names a blank group "New group"', () => {
    expect(freeGroupName(template('blank'), [])).toBe('New group');
  });
});
