/**
 * Builder for the "Actions" submenu shown in a card's more menu: one group
 * per action type (separated), with pinned actions excluded (they have their
 * own buttons in the card header) and Format & Check collapsed into a nested
 * submenu when they'd crowd the list. The MenuItem shape is also used by other
 * submenu builders (e.g. the branch card's Open In menu).
 */

import Wand2 from '@lucide/svelte/icons/wand-2';
import type { ProjectAction } from '../../api/commands';
import type { ActionType } from './actions';
import { getActionTypeIcon, type IconComponent } from './lucideIcons';

export type MenuIconComponent = IconComponent;
export type ActionMenuItem = {
  type: 'action';
  label: string;
  icon?: MenuIconComponent;
  iconSrc?: string;
  disabled?: boolean;
  danger?: boolean;
  onSelect: () => void | Promise<void>;
};
export type SeparatorMenuItem = { type: 'separator' };
export type SubmenuMenuItem = {
  type: 'submenu';
  label: string;
  icon?: MenuIconComponent;
  disabled?: boolean;
  children: MenuItem[];
};
export type MenuItem = ActionMenuItem | SeparatorMenuItem | SubmenuMenuItem;

const actionMenuTypes = ['run', 'build', 'format', 'check', 'test', 'cleanUp', 'prerun'] as const;

/**
 * Build the menu items for a scope's actions, leaving out every action already
 * pinned to the card header.
 */
export function buildActionMenuItems(
  groupedActions: Record<string, ProjectAction[]>,
  pinnedActionIds: Set<string>,
  onRun: (action: ProjectAction) => void | Promise<void>
): MenuItem[] {
  const toActionItem = (type: ActionType, action: ProjectAction): MenuItem => ({
    type: 'action',
    label: action.name,
    icon: getActionTypeIcon(type),
    onSelect: () => onRun(action),
  });
  const unpinned = (type: ActionType): ProjectAction[] =>
    (groupedActions[type] ?? []).filter((a) => !pinnedActionIds.has(a.id));

  const formatItems = unpinned('format').map((a) => toActionItem('format', a));
  const checkItems = unpinned('check').map((a) => toActionItem('check', a));
  const combineFormatCheck = formatItems.length + checkItems.length > 2;

  const groups: MenuItem[][] = [];
  for (const type of actionMenuTypes) {
    if (combineFormatCheck && type === 'check') continue;
    if (combineFormatCheck && type === 'format') {
      const children: MenuItem[] = [
        ...formatItems,
        ...(formatItems.length && checkItems.length ? [{ type: 'separator' as const }] : []),
        ...checkItems,
      ];
      groups.push([
        {
          type: 'submenu',
          label: 'Format & Check',
          icon: Wand2,
          children,
        },
      ]);
      continue;
    }

    const typeActions = unpinned(type);
    if (typeActions.length === 0) continue;
    groups.push(typeActions.map((action) => toActionItem(type, action)));
  }

  const items: MenuItem[] = [];
  for (const group of groups) {
    if (items.length > 0) items.push({ type: 'separator' });
    items.push(...group);
  }
  return items;
}
