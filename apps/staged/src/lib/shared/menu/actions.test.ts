import { describe, expect, it, vi } from 'vitest';
import { selectMenuAction } from './actions';
import type { MenuActionItem } from './types';

function actionItem(overrides: Partial<MenuActionItem> = {}): MenuActionItem {
  return {
    type: 'action',
    label: 'Action',
    onSelect: vi.fn(),
    ...overrides,
  };
}

describe('selectMenuAction', () => {
  it('runs the action before closing the menu', () => {
    const calls: string[] = [];
    const item = actionItem({
      onSelect: () => {
        calls.push('select');
      },
    });

    selectMenuAction(item, () => calls.push('close'));

    expect(calls).toEqual(['select', 'close']);
  });

  it('does not close when closeOnSelect is false', () => {
    const onClose = vi.fn();
    const item = actionItem({ closeOnSelect: false });

    selectMenuAction(item, onClose);

    expect(item.onSelect).toHaveBeenCalledOnce();
    expect(onClose).not.toHaveBeenCalled();
  });

  it('ignores disabled items', () => {
    const onClose = vi.fn();
    const item = actionItem({ disabled: true });

    selectMenuAction(item, onClose);

    expect(item.onSelect).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });
});
