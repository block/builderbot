import { describe, expect, it } from 'vitest';
import type { ProjectAction } from '../../api/commands';
import { getPinnedActions, getSecondaryRunningActions, groupActionsByType } from './actionGroups';

function action(
  name: string,
  overrides: Partial<ProjectAction> & Pick<ProjectAction, 'actionType' | 'sortOrder'>
): ProjectAction {
  return {
    id: `action-${name}`,
    contextId: 'context-1',
    name,
    command: name,
    autoCommit: false,
    pinned: false,
    icon: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

describe('getPinnedActions', () => {
  it('returns every pinned action in sort order, whatever its type', () => {
    const actions = [
      action('Test', { actionType: 'test', sortOrder: 3, pinned: true, icon: 'flask-conical' }),
      action('Build', { actionType: 'build', sortOrder: 2 }),
      action('Dev', { actionType: 'run', sortOrder: 1, pinned: true }),
      action('Storybook', { actionType: 'run', sortOrder: 0, pinned: true, icon: 'palette' }),
    ];

    expect(getPinnedActions(actions).map((a) => a.name)).toEqual(['Storybook', 'Dev', 'Test']);
  });

  it('is empty when nothing is pinned — a header with no action buttons', () => {
    const actions = [
      action('Dev', { actionType: 'run', sortOrder: 0 }),
      action('Test', { actionType: 'test', sortOrder: 1 }),
    ];

    expect(getPinnedActions(actions)).toEqual([]);
  });
});

describe('getSecondaryRunningActions', () => {
  const running = [
    { actionId: 'action-Dev' },
    { actionId: 'action-Test' },
    { actionId: 'action-Build' },
  ];

  it('drops every pinned action, not just one', () => {
    const pinned = new Set(['action-Dev', 'action-Build']);
    expect(getSecondaryRunningActions(running, pinned)).toEqual([{ actionId: 'action-Test' }]);
  });

  it('keeps everything when nothing is pinned', () => {
    expect(getSecondaryRunningActions(running, new Set())).toEqual(running);
  });
});

describe('groupActionsByType', () => {
  it('buckets actions by type and leaves unknown types out', () => {
    const groups = groupActionsByType([
      action('Dev', { actionType: 'run', sortOrder: 0 }),
      action('Test', { actionType: 'test', sortOrder: 1 }),
      action('Mystery', { actionType: 'nonsense' as ProjectAction['actionType'], sortOrder: 2 }),
    ]);

    expect(groups.run.map((a) => a.name)).toEqual(['Dev']);
    expect(groups.test.map((a) => a.name)).toEqual(['Test']);
    expect(
      Object.values(groups)
        .flat()
        .map((a) => a.name)
    ).not.toContain('Mystery');
  });
});
