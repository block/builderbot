import { describe, expect, it, vi } from 'vitest';
import type { ProjectAction } from '../../api/commands';

// Vitest runs without the Svelte plugin, so the icon components the builder
// hangs off each item can't be imported here — and this suite only cares which
// items it produces.
vi.mock('@lucide/svelte/icons/wand-2', () => ({ default: 'Wand2' }));
vi.mock('./lucideIcons', () => ({ getActionTypeIcon: (type: string) => `icon:${type}` }));

const { buildActionMenuItems } = await import('./actionMenu');
const { groupActionsByType } = await import('./actionGroups');

function action(name: string, actionType: string, pinned = false): ProjectAction {
  return {
    id: `action-${name}`,
    contextId: 'context-1',
    name,
    command: name,
    actionType,
    sortOrder: 0,
    autoCommit: false,
    pinned,
    icon: null,
    createdAt: 0,
    updatedAt: 0,
  };
}

function labels(items: ReturnType<typeof buildActionMenuItems>): string[] {
  return items.flatMap((item) => (item.type === 'action' ? [item.label] : []));
}

describe('buildActionMenuItems', () => {
  it('leaves out pinned actions of every type, not just run actions', () => {
    const actions = [
      action('Dev', 'run', true),
      action('Storybook', 'run'),
      action('Test', 'test', true),
      action('Build', 'build'),
    ];
    const pinned = new Set(actions.filter((a) => a.pinned).map((a) => a.id));

    const items = buildActionMenuItems(groupActionsByType(actions), pinned, vi.fn());

    // Pinned actions have their own header buttons, so listing them here too
    // would just be a second way to press the same thing.
    expect(labels(items)).toEqual(['Storybook', 'Build']);
  });

  it('lists everything when nothing is pinned', () => {
    const actions = [action('Dev', 'run'), action('Build', 'build')];

    const items = buildActionMenuItems(groupActionsByType(actions), new Set(), vi.fn());

    expect(labels(items)).toEqual(['Dev', 'Build']);
  });

  it('counts only unpinned format/check actions toward the collapse threshold', () => {
    const actions = [
      action('Fmt', 'format', true),
      action('Lint', 'check'),
      action('Types', 'check'),
    ];
    const pinned = new Set(['action-Fmt']);

    const items = buildActionMenuItems(groupActionsByType(actions), pinned, vi.fn());

    // Two remaining entries don't crowd the list, so they stay inline rather
    // than getting folded into a "Format & Check" submenu.
    expect(items.some((item) => item.type === 'submenu')).toBe(false);
    expect(labels(items)).toEqual(['Lint', 'Types']);
  });

  it('runs the action a selected item stands for', async () => {
    const onRun = vi.fn();
    const dev = action('Dev', 'run');

    const items = buildActionMenuItems(groupActionsByType([dev]), new Set(), onRun);
    const item = items[0];
    if (item.type !== 'action') throw new Error('expected an action item');
    await item.onSelect();

    expect(onRun).toHaveBeenCalledWith(dev);
  });
});
