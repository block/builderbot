// dialogDefaultAction.ts — plain Return on a dialog activates its default button.

import { isImeKeydown } from './imeKeydown';

/**
 * Controls that take Return themselves: a focused button or link activates
 * on it, a field submits or breaks a line, a widget picks its item. The
 * dialog leaves their keys alone, or a focused Cancel would both cancel and
 * confirm.
 */
const TAKES_RETURN_SELECTOR = [
  'button',
  'a[href]',
  'input',
  'select',
  'textarea',
  'summary',
  '[contenteditable]:not([contenteditable="false"])',
  ...[
    'button',
    'link',
    'checkbox',
    'radio',
    'switch',
    'tab',
    'option',
    'menuitem',
    'menuitemcheckbox',
    'menuitemradio',
    'treeitem',
    'combobox',
    'textbox',
    'searchbox',
    'slider',
    'spinbutton',
  ].map((role) => `[role="${role}"]`),
].join(', ');

/**
 * Whether `e` is a plain Return for the dialog's default button: no
 * modifier, not a held key repeating (the press that opened the dialog must
 * not also confirm it), not an IME commit, not already handled, and not
 * aimed at a control with its own use for Return.
 */
export function isDefaultActionKeydown(e: KeyboardEvent): boolean {
  if (e.key !== 'Enter') return false;
  if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return false;
  if (e.repeat || e.defaultPrevented || isImeKeydown(e)) return false;
  const target = e.target;
  return !(target instanceof Element && target.closest(TAKES_RETURN_SELECTOR) !== null);
}

/**
 * The button matching `selector` inside `container`, if it can be pressed:
 * null when there is none or it is disabled, so a busy dialog stays busy.
 */
export function findEnabledButton(container: ParentNode, selector: string): HTMLElement | null {
  const button = container.querySelector<HTMLElement>(selector);
  if (!button) return null;
  if ((button as HTMLButtonElement).disabled || button.getAttribute('aria-disabled') === 'true') {
    return null;
  }
  return button;
}
