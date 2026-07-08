// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from 'vitest';
import { focusInitialAcpPickerColumn, handleAcpPickerGridKeydown } from './acpPickerKeyboard';

describe('acp picker keyboard navigation', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('starts focus in the model column', () => {
    const root = buildPicker();

    expect(focusInitialAcpPickerColumn(root)).toBe(true);
    expect(document.activeElement?.textContent).toBe('Model B');
  });

  it('keeps up and down navigation inside the active column', () => {
    const root = buildPicker();
    listenForPickerKeys(root);
    getItem(root, 'Model A').focus();

    expect(pressKey('ArrowUp')).toBe(true);
    expect(document.activeElement?.textContent).toBe('Model A');

    getItem(root, 'Model C').focus();

    expect(pressKey('ArrowDown')).toBe(true);
    expect(document.activeElement?.textContent).toBe('Model C');
  });

  it('moves vertically within a column when there is another row', () => {
    const root = buildPicker();
    listenForPickerKeys(root);
    getItem(root, 'Model B').focus();

    expect(pressKey('ArrowUp')).toBe(true);
    expect(document.activeElement?.textContent).toBe('Model A');

    expect(pressKey('ArrowDown')).toBe(true);
    expect(document.activeElement?.textContent).toBe('Model B');
  });

  it('activates values as up and down move through a column', () => {
    const root = buildPicker();
    const clicked: string[] = [];
    trackClicks(root, clicked);
    listenForPickerKeys(root);
    getItem(root, 'Model B').focus();

    expect(pressKey('ArrowUp')).toBe(true);
    expect(document.activeElement?.textContent).toBe('Model A');
    expect(clicked).toEqual(['Model A']);

    expect(pressKey('ArrowDown')).toBe(true);
    expect(document.activeElement?.textContent).toBe('Model B');
    expect(clicked).toEqual(['Model A', 'Model B']);
  });

  it('moves left and right between columns', () => {
    const root = buildPicker();
    listenForPickerKeys(root);
    getItem(root, 'Model A').focus();

    expect(pressKey('ArrowRight')).toBe(true);
    expect(document.activeElement?.textContent).toBe('Effort High');

    expect(pressKey('ArrowLeft')).toBe(true);
    expect(document.activeElement?.textContent).toBe('Model B');
  });

  it('dismisses on return without activating the focused item', () => {
    const root = buildPicker();
    const clicked: string[] = [];
    let dismissed = false;
    trackClicks(root, clicked);
    listenForPickerKeys(root, () => {
      dismissed = true;
    });
    getItem(root, 'Model B').focus();

    expect(pressKey('Enter')).toBe(true);
    expect(dismissed).toBe(true);
    expect(clicked).toEqual([]);
  });
});

function buildPicker(): HTMLElement {
  const root = document.createElement('div');
  root.append(
    column('provider', [
      item('Codex'),
      item('Claude', {
        checked: true,
      }),
    ]),
    column('model', [item('Model A'), item('Model B', { checked: true }), item('Model C')]),
    column('effort', [item('Effort Low'), item('Effort High', { checked: true })])
  );
  document.body.append(root);
  return root;
}

function column(kind: string, items: HTMLElement[]): HTMLElement {
  const element = document.createElement('div');
  element.className = 'picker-column';
  element.dataset.pickerColumn = kind;
  element.append(...items);
  return element;
}

function item(label: string, options: { checked?: boolean } = {}): HTMLElement {
  const element = document.createElement('div');
  element.dataset.slot = 'dropdown-menu-radio-item';
  element.tabIndex = -1;
  element.textContent = label;
  if (options.checked) {
    element.setAttribute('aria-checked', 'true');
  }
  return element;
}

function listenForPickerKeys(root: HTMLElement, onDismiss?: () => void): void {
  root.addEventListener(
    'keydown',
    (event) => {
      handleAcpPickerGridKeydown(event, root, { onDismiss });
    },
    { capture: true }
  );
}

function pressKey(key: string): boolean {
  const activeElement = document.activeElement;
  if (!(activeElement instanceof HTMLElement)) {
    throw new Error('Expected an active element');
  }

  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
  activeElement.dispatchEvent(event);
  return event.defaultPrevented;
}

function getItem(root: HTMLElement, label: string): HTMLElement {
  const item = Array.from(
    root.querySelectorAll<HTMLElement>("[data-slot='dropdown-menu-radio-item']")
  ).find((candidate) => candidate.textContent === label);

  if (!item) {
    throw new Error(`Missing item: ${label}`);
  }

  return item;
}

function trackClicks(root: HTMLElement, clicked: string[]): void {
  root.querySelectorAll<HTMLElement>("[data-slot='dropdown-menu-radio-item']").forEach((item) => {
    item.addEventListener('click', () => {
      clicked.push(item.textContent ?? '');
    });
  });
}
