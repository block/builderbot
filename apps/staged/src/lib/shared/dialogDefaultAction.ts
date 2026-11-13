// dialogDefaultAction.ts — plain Return on a dialog activates its default button.

import { isImeKeydown } from './imeKeydown';

/**
 * Controls that take Return themselves: a focused button or link activates
 * on it, a field submits or breaks a line, a widget picks its item. The
 * dialog leaves their keys alone, or a focused Cancel would both cancel and
 * confirm.
 *
 * The composite roles at the end are containers that can hold the focus for
 * their items (a listbox tracking its option through `aria-activedescendant`,
 * a menu, a grid) and panels with their own Return (a dialog docked inside
 * this one). A Return on such a container, or on anything in it, is a pick
 * or a confirmation there, never this dialog's default button.
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
    'listbox',
    'menu',
    'menubar',
    'grid',
    'treegrid',
    'tree',
    'radiogroup',
    'tablist',
    'dialog',
    'alertdialog',
  ].map((role) => `[role="${role}"]`),
].join(', ');

/**
 * Whether `e` is a plain Return for the dialog's default button: no
 * modifier, not a held key repeating (the press that opened the dialog must
 * not also confirm it), not an IME commit, not already handled, and not
 * aimed at a control with its own use for Return.
 *
 * Call it from the dialog's own keydown listener: the controls looked for
 * are the target and what lies between it and the listening element
 * (`e.currentTarget`). The listening element is left out, since the dialog
 * carries `role="dialog"` itself.
 */
export function isDefaultActionKeydown(e: KeyboardEvent): boolean {
  if (e.key !== 'Enter') return false;
  if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return false;
  if (e.repeat || e.defaultPrevented || isImeKeydown(e)) return false;
  return !(e.target instanceof Element && takesReturn(e.target, e.currentTarget));
}

/** Whether `target`, or an element between it and `listener`, takes Return itself. */
function takesReturn(target: Element, listener: EventTarget | null): boolean {
  for (let el: Element | null = target; el && el !== listener; el = el.parentElement) {
    if (el.matches(TAKES_RETURN_SELECTOR)) return true;
  }
  return false;
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

/** A button written as a submit button: `type="submit"` spelled out. */
const EXPLICIT_SUBMIT_SELECTOR = 'button[type="submit" i], input[type="submit" i]';

/**
 * The form's default button: the first button that belongs to the form,
 * inside it or tied to it by a `form` attribute, and says `type="submit"`.
 *
 * The browser, for a Return typed in a field, also counts a button with no
 * `type`, which submits by default. This does not. An untyped button in a
 * form is nearly always a chip or an option row whose author never meant it
 * to submit, and pressing one from the keyboard runs its click handler
 * first: Return on the Add Repository dialog used to pick the first recent
 * repo and add it. So every button in a form states its type, and one that
 * does not is never pressed from here.
 */
function defaultSubmitButton(form: HTMLFormElement): HTMLElement | null {
  for (const control of Array.from(form.elements)) {
    if (control.matches(EXPLICIT_SUBMIT_SELECTOR)) return control as HTMLElement;
  }
  return null;
}

/**
 * Editors where Return breaks a line. A disabled one (`contenteditable`
 * turned "false" while the form is busy) still marks its form as multiline.
 */
const MULTILINE_EDITOR_SELECTOR = 'textarea, [contenteditable]';

/**
 * The form a plain Return submits in a dialog: its first form with an
 * explicit submit button (see `defaultSubmitButton`), whether or not that
 * button can be pressed right now. Null when there is none (a viewer has
 * nothing to submit), and, unless `allowMultiline`, when the form holds a
 * multiline editor: there Return breaks a line and Cmd+Return submits, so
 * plain Return submits from nowhere in the dialog.
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
 * The button a Return submits `form` with, its default button (see
 * `defaultSubmitButton`), or null when that is disabled: the browser
 * submits nothing then either.
 */
export function findEnabledSubmitButton(form: HTMLFormElement): HTMLElement | null {
  const button = defaultSubmitButton(form);
  return button && isPressable(button) ? button : null;
}
