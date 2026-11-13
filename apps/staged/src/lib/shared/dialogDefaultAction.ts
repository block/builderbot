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
  return button && isPressable(button) ? button : null;
}

function isPressable(button: HTMLElement): boolean {
  return !(button as HTMLButtonElement).disabled && button.getAttribute('aria-disabled') !== 'true';
}

/**
 * The form's default button, as the browser picks it for a Return typed in
 * one of its fields: the first submit button that belongs to the form,
 * inside it or tied to it by a `form` attribute. A button with no `type`
 * submits, so it counts.
 */
function defaultSubmitButton(form: HTMLFormElement): HTMLElement | null {
  for (const control of Array.from(form.elements)) {
    const type = (control as HTMLButtonElement | HTMLInputElement).type;
    if (type === 'submit' && control.matches('button, input')) return control as HTMLElement;
  }
  return null;
}

/**
 * Editors where Return breaks a line. A disabled one (`contenteditable`
 * turned "false" while the form is busy) still marks its form as multiline.
 */
const MULTILINE_EDITOR_SELECTOR = 'textarea, [contenteditable]';

/**
 * The form a plain Return submits in a dialog: its first form with a submit
 * button, whether or not that button can be pressed right now. Null when
 * there is none (a viewer has nothing to submit), and, unless
 * `allowMultiline`, when the form holds a multiline editor: there Return
 * breaks a line and Cmd+Return submits, so plain Return submits from nowhere
 * in the dialog.
 */
export function findReturnSubmitForm(
  container: ParentNode,
  allowMultiline = false
): HTMLFormElement | null {
  const form = container.querySelector('form');
  if (!form || !defaultSubmitButton(form)) return null;
  if (!allowMultiline && form.querySelector(MULTILINE_EDITOR_SELECTOR)) return null;
  return form;
}

/**
 * The button a Return in one of `form`'s fields would press, its default
 * button, or null when that is disabled: the browser submits nothing then
 * either.
 */
export function findEnabledSubmitButton(form: HTMLFormElement): HTMLElement | null {
  const button = defaultSubmitButton(form);
  return button && isPressable(button) ? button : null;
}
