// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { findEnabledButton, isDefaultActionKeydown } from './dialogDefaultAction';

let dialog: HTMLElement;

beforeEach(() => {
  dialog = document.createElement('div');
  dialog.setAttribute('role', 'alertdialog');
  dialog.tabIndex = -1;
  document.body.appendChild(dialog);
});

afterEach(() => {
  dialog.remove();
});

/** Dispatch a keydown on `target` and return what the dialog's handler decided. */
function decide(target: Element, init: KeyboardEventInit = {}, prevent = false): boolean {
  let decided: boolean | null = null;
  const listener = (e: KeyboardEvent) => {
    if (prevent) e.preventDefault();
    decided = isDefaultActionKeydown(e);
  };
  dialog.addEventListener('keydown', listener);
  target.dispatchEvent(
    new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true, ...init })
  );
  dialog.removeEventListener('keydown', listener);
  if (decided === null) throw new Error('keydown did not reach the dialog');
  return decided;
}

function child(html: string): Element {
  dialog.innerHTML = html;
  return dialog.firstElementChild!;
}

describe('isDefaultActionKeydown', () => {
  it('accepts a plain Return on the dialog itself', () => {
    expect(decide(dialog)).toBe(true);
  });

  it('accepts a plain Return on text inside the dialog', () => {
    expect(decide(child('<p>Are you sure?</p>'))).toBe(true);
  });

  it('rejects other keys', () => {
    expect(decide(dialog, { key: ' ' })).toBe(false);
    expect(decide(dialog, { key: 'Escape' })).toBe(false);
  });

  it.each(['metaKey', 'ctrlKey', 'altKey', 'shiftKey'] as const)('rejects %s+Return', (mod) => {
    expect(decide(dialog, { [mod]: true })).toBe(false);
  });

  it('rejects a held key repeating', () => {
    expect(decide(dialog, { repeat: true })).toBe(false);
  });

  it('rejects a Return that commits an IME composition', () => {
    expect(decide(dialog, { isComposing: true })).toBe(false);
    // Safari's committing Return, after compositionend.
    expect(decide(dialog, { keyCode: 229 })).toBe(false);
  });

  it('rejects a Return another handler already took', () => {
    expect(decide(dialog, {}, true)).toBe(false);
  });

  it.each([
    ['a button', '<button type="button">Cancel</button>'],
    ['a link', '<a href="https://example.com">Docs</a>'],
    ['a text field', '<input type="text" />'],
    ['a radio', '<input type="radio" />'],
    ['a select', '<select><option>One</option></select>'],
    ['a textarea', '<textarea></textarea>'],
    ['editable content', '<div contenteditable="true"></div>'],
    ['a widget role', '<div role="option" tabindex="0">One</div>'],
  ])('rejects a Return on %s', (_name, html) => {
    expect(decide(child(html))).toBe(false);
  });

  it('rejects a Return on an element inside a button', () => {
    const button = child('<button type="button"><span>Cancel</span></button>');
    expect(decide(button.firstElementChild!)).toBe(false);
  });

  it('accepts a Return on content marked not editable', () => {
    expect(decide(child('<div contenteditable="false" tabindex="0">Read only</div>'))).toBe(true);
  });
});

describe('findEnabledButton', () => {
  it('finds the matching button', () => {
    dialog.innerHTML =
      '<button data-slot="cancel">No</button><button data-slot="action">Yes</button>';
    expect(findEnabledButton(dialog, '[data-slot="action"]')?.textContent).toBe('Yes');
  });

  it('is null when there is no such button', () => {
    dialog.innerHTML = '<button data-slot="cancel">No</button>';
    expect(findEnabledButton(dialog, '[data-slot="action"]')).toBeNull();
  });

  it('is null when the button is disabled', () => {
    dialog.innerHTML = '<button data-slot="action" disabled>Yes</button>';
    expect(findEnabledButton(dialog, '[data-slot="action"]')).toBeNull();
  });

  it('is null when the button is aria-disabled', () => {
    dialog.innerHTML = '<a data-slot="action" role="link" aria-disabled="true">Yes</a>';
    expect(findEnabledButton(dialog, '[data-slot="action"]')).toBeNull();
  });
});
