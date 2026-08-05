/**
 * Builder for the "Actions" submenu shown in a card's more menu: one group
 * per action type (separated), with the primary run action excluded (it has
 * its own button) and Format & Check collapsed into a nested submenu when
 * they'd crowd the list. The MenuItem shape is also used by other submenu
 * builders (e.g. the branch card's Open In menu).
 */

import Play from '@lucide/svelte/icons/play';
import Hammer from '@lucide/svelte/icons/hammer';
import FlaskConical from '@lucide/svelte/icons/flask-conical';
import CheckCircle from '@lucide/svelte/icons/check-circle';
import Wrench from '@lucide/svelte/icons/wrench';
import Zap from '@lucide/svelte/icons/zap';
import Wand2 from '@lucide/svelte/icons/wand-2';
import type { ProjectAction } from '../../api/commands';
import type { ActionType } from './actions';

export type MenuIconComponent = typeof Play;
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

export function getActionIcon(actionType: string): MenuIconComponent {
  switch (actionType) {
    case 'prerun':
      return Zap;
    case 'run':
      return Play;
    case 'build':
      return Hammer;
    case 'format':
      return Wand2;
    case 'check':
      return CheckCircle;
    case 'test':
      return FlaskConical;
    case 'cleanUp':
      return Wrench;
    default:
      return Wrench;
  }
}

export function buildActionMenuItems(
  groupedActions: Record<string, ProjectAction[]>,
  remainingRunActions: ProjectAction[],
  onRun: (action: ProjectAction) => void | Promise<void>
): MenuItem[] {
  const toActionItem = (type: ActionType, action: ProjectAction): MenuItem => ({
    type: 'action',
    label: action.name,
    icon: getActionIcon(type),
    onSelect: () => onRun(action),
  });

  const formatItems = groupedActions.format.map((a) => toActionItem('format', a));
  const checkItems = groupedActions.check.map((a) => toActionItem('check', a));
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

    const typeActions = type === 'run' ? remainingRunActions : groupedActions[type];
    if (!typeActions || typeActions.length === 0) continue;
    groups.push(typeActions.map((action) => toActionItem(type, action)));
  }

  const items: MenuItem[] = [];
  for (const group of groups) {
    if (items.length > 0) items.push({ type: 'separator' });
    items.push(...group);
  }
  return items;
}
