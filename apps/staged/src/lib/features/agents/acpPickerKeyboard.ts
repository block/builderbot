const PICKER_COLUMN_SELECTOR = '.picker-column';
const PICKER_ITEM_SELECTOR =
  "[data-slot='dropdown-menu-radio-item']:not([data-disabled]), [data-slot='dropdown-menu-item']:not([data-disabled])";
const CHECKED_ITEM_SELECTOR = "[aria-checked='true']";

type PickerDirection = 'previous' | 'next';

interface PickerKeydownOptions {
  onDismiss?: () => void;
}

export function handleAcpPickerOpenAutoFocus(event: Event, root: HTMLElement | null): void {
  event.preventDefault();
  window.setTimeout(() => focusInitialAcpPickerColumn(root), 0);
}

export function handleAcpPickerGridKeydown(
  event: KeyboardEvent,
  root: HTMLElement | null,
  options: PickerKeydownOptions = {}
): boolean {
  if (event.defaultPrevented || !root) return false;

  if (event.key === 'Enter') {
    event.preventDefault();
    event.stopPropagation();
    options.onDismiss?.();
    return true;
  }

  if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
    return focusVertical(event, root, event.key === 'ArrowUp' ? 'previous' : 'next');
  }

  if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
    return focusHorizontal(event, root, event.key === 'ArrowLeft' ? 'previous' : 'next');
  }

  return false;
}

export function focusInitialAcpPickerColumn(root: HTMLElement | null): boolean {
  if (!root) return false;

  const columns = getFocusableColumns(root);
  const column =
    columns.find((candidate) => candidate.dataset.pickerColumn === 'model') ??
    columns.find((candidate) => candidate.dataset.pickerColumn !== 'provider') ??
    columns[0];

  return focusPreferredItem(column);
}

function focusVertical(
  event: KeyboardEvent,
  root: HTMLElement,
  direction: PickerDirection
): boolean {
  const current = getCurrentColumnState(root, event.target);
  if (!current) return false;

  const nextIndex =
    direction === 'previous'
      ? Math.max(0, current.itemIndex - 1)
      : Math.min(current.items.length - 1, current.itemIndex + 1);

  event.preventDefault();
  event.stopPropagation();
  focusAndActivateItem(current.items[nextIndex]);
  return true;
}

function focusHorizontal(
  event: KeyboardEvent,
  root: HTMLElement,
  direction: PickerDirection
): boolean {
  const current = getCurrentColumnState(root, event.target);
  const columns = getFocusableColumns(root);
  if (!current || columns.length === 0) return false;

  const columnIndex = columns.indexOf(current.column);
  const nextColumnIndex =
    direction === 'previous'
      ? Math.max(0, columnIndex - 1)
      : Math.min(columns.length - 1, columnIndex + 1);
  const nextColumn = columns[nextColumnIndex];

  event.preventDefault();
  event.stopPropagation();
  focusPreferredItem(nextColumn, current.itemIndex);
  return true;
}

function getCurrentColumnState(root: HTMLElement, target: EventTarget | null) {
  const targetElement = target instanceof Element ? target : null;
  const activeElement = root.ownerDocument.activeElement;
  const focusedElement =
    targetElement && root.contains(targetElement)
      ? targetElement
      : activeElement instanceof Element && root.contains(activeElement)
        ? activeElement
        : null;
  const column = focusedElement?.closest<HTMLElement>(PICKER_COLUMN_SELECTOR);
  const fallbackColumn = focusInitialColumn(root);
  const currentColumn = column && root.contains(column) ? column : fallbackColumn;
  if (!currentColumn) return null;

  const items = getColumnItems(currentColumn);
  if (items.length === 0) return null;

  const focusedItem = focusedElement?.closest<HTMLElement>(PICKER_ITEM_SELECTOR);
  const itemIndex = focusedItem ? items.indexOf(focusedItem) : -1;

  return {
    column: currentColumn,
    items,
    itemIndex: itemIndex >= 0 ? itemIndex : preferredItemIndex(items),
  };
}

function focusInitialColumn(root: HTMLElement): HTMLElement | null {
  const columns = getFocusableColumns(root);
  return (
    columns.find((candidate) => candidate.dataset.pickerColumn === 'model') ??
    columns.find((candidate) => candidate.dataset.pickerColumn !== 'provider') ??
    columns[0] ??
    null
  );
}

function getFocusableColumns(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(PICKER_COLUMN_SELECTOR)).filter(
    (column) => getColumnItems(column).length > 0
  );
}

function getColumnItems(column: HTMLElement): HTMLElement[] {
  return Array.from(column.querySelectorAll<HTMLElement>(PICKER_ITEM_SELECTOR));
}

function focusPreferredItem(column: HTMLElement | undefined, fallbackIndex = 0): boolean {
  if (!column) return false;

  const items = getColumnItems(column);
  if (items.length === 0) return false;

  const checkedItem = column.querySelector<HTMLElement>(CHECKED_ITEM_SELECTOR);
  const item =
    checkedItem && items.includes(checkedItem)
      ? checkedItem
      : items[Math.min(Math.max(fallbackIndex, 0), items.length - 1)];

  focusItem(item);
  return true;
}

function preferredItemIndex(items: HTMLElement[]): number {
  const checkedIndex = items.findIndex((item) => item.matches(CHECKED_ITEM_SELECTOR));
  return checkedIndex >= 0 ? checkedIndex : 0;
}

function focusItem(item: HTMLElement | undefined): void {
  item?.focus({ preventScroll: true });
}

function focusAndActivateItem(item: HTMLElement | undefined): void {
  if (!item) return;
  focusItem(item);
  item.click();
}
