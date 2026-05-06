import type { MenuActionItem } from './types';

export function selectMenuAction(item: MenuActionItem, onClose: () => void): void {
  if (item.disabled) return;

  let result: void | Promise<void>;
  try {
    result = item.onSelect();
  } finally {
    if (item.closeOnSelect !== false) {
      onClose();
    }
  }

  void result;
}
