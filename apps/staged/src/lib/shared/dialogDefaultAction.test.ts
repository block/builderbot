// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  findEnabledButton,
  findEnabledSubmitButton,
  findReturnSubmitForm,
  isDefaultActionKeydown,
} from './dialogDefaultAction';

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

  // Containers that hold the focus for their items, and panels with their
  // own Return, docked inside the dialog.
  it.each([
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
  ])('rejects a Return on a focused %s container', (role) => {
    expect(decide(child(`<div role="${role}" tabindex="-1"></div>`))).toBe(false);
  });

  it('rejects a Return on a plain element inside such a container', () => {
    const listbox = child('<div role="listbox"><div tabindex="-1">Loading…</div></div>');
    expect(decide(listbox.firstElementChild!)).toBe(false);

    const panel = child('<div role="dialog"><section><p tabindex="-1">Reply</p></section></div>');
    expect(decide(panel.querySelector('p')!)).toBe(false);
  });

  it('accepts a Return beside such a container, not inside it', () => {
    dialog.innerHTML = '<div role="listbox" tabindex="-1"></div><p tabindex="-1">Pick one</p>';
    expect(decide(dialog.querySelector('p')!)).toBe(true);
  });

  it('does not count the listening dialog itself, or what it sits in', () => {
    // `dialog` has role="alertdialog"; put it inside a menu and a dialog too.
    const outer = document.createElement('div');
    outer.setAttribute('role', 'menu');
    const outerDialog = document.createElement('div');
    outerDialog.setAttribute('role', 'dialog');
    outer.appendChild(outerDialog);
    document.body.appendChild(outer);
    outerDialog.appendChild(dialog);
    try {
      expect(decide(dialog)).toBe(true);
      expect(decide(child('<p>Are you sure?</p>'))).toBe(true);
      expect(decide(child('<button type="button">Cancel</button>'))).toBe(false);
    } finally {
      document.body.appendChild(dialog);
      outer.remove();
    }
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

describe('findReturnSubmitForm', () => {
  it('finds a form with a submit button', () => {
    dialog.innerHTML = '<form><input /><button type="submit">Move</button></form>';
    expect(findReturnSubmitForm(dialog)).toBe(dialog.querySelector('form'));
  });

  it('finds it while the submit button is disabled', () => {
    dialog.innerHTML = '<form><button type="submit" disabled>Move</button></form>';
    expect(findReturnSubmitForm(dialog)).toBe(dialog.querySelector('form'));
  });

  it('finds a form whose submit button is an input, or spells its type in capitals', () => {
    dialog.innerHTML = '<form><input type="submit" value="Move" /></form>';
    expect(findReturnSubmitForm(dialog)).not.toBeNull();
    dialog.innerHTML = '<form><button type="SUBMIT">Move</button></form>';
    expect(findReturnSubmitForm(dialog)).not.toBeNull();
  });

  it('is null when the only button has no type: it submits, but was not written to', () => {
    dialog.innerHTML = '<form><input /><button>Chip</button></form>';
    expect(findReturnSubmitForm(dialog)).toBeNull();
  });

  it('is null when the type is misspelt, which the browser also takes for submit', () => {
    dialog.innerHTML = '<form><input /><button type="sumbit">Move</button></form>';
    expect((dialog.querySelector('button') as HTMLButtonElement).type).toBe('submit');
    expect(findReturnSubmitForm(dialog)).toBeNull();
  });

  it('is null when the dialog has no form', () => {
    dialog.innerHTML = '<p>Something to read.</p><button type="button">Close</button>';
    expect(findReturnSubmitForm(dialog)).toBeNull();
  });

  it('is null when the form has nothing that submits it', () => {
    dialog.innerHTML = '<form><input /><button type="button">Cancel</button></form>';
    expect(findReturnSubmitForm(dialog)).toBeNull();
  });

  it.each([
    ['a textarea', '<textarea></textarea>'],
    ['editable content', '<div contenteditable="true"></div>'],
    ['editable content turned off while busy', '<div contenteditable="false"></div>'],
  ])('is null when the form holds %s', (_name, editor) => {
    dialog.innerHTML = `<form>${editor}<button type="submit">Start</button></form>`;
    expect(findReturnSubmitForm(dialog)).toBeNull();
  });

  it('finds a multiline form when asked to allow one', () => {
    dialog.innerHTML = '<form><textarea></textarea><button type="submit">Start</button></form>';
    expect(findReturnSubmitForm(dialog, true)).toBe(dialog.querySelector('form'));
  });

  it('ignores a multiline editor outside the form', () => {
    dialog.innerHTML =
      '<textarea></textarea><form><input /><button type="submit">Move</button></form>';
    expect(findReturnSubmitForm(dialog)).not.toBeNull();
  });
});

describe('findEnabledSubmitButton', () => {
  function form(html: string): HTMLFormElement {
    dialog.innerHTML = `<form>${html}</form>`;
    return dialog.querySelector('form')!;
  }

  it('finds the submit button, past the other buttons', () => {
    const found = findEnabledSubmitButton(
      form('<button type="button">Cancel</button><button type="submit">Move</button>')
    );
    expect(found?.textContent).toBe('Move');
  });

  it('is null when the submit button is disabled', () => {
    expect(
      findEnabledSubmitButton(form('<button type="submit" disabled>Move</button>'))
    ).toBeNull();
  });

  it('finds a submit button outside the form that is tied to it by a form attribute', () => {
    dialog.innerHTML =
      '<form id="f"><input /></form><button>Chip</button><button type="submit" form="f">Create</button>';
    const form = dialog.querySelector('form')!;
    expect(findReturnSubmitForm(dialog)).toBe(form);
    expect(findEnabledSubmitButton(form)?.textContent).toBe('Create');
  });

  it('passes over a button with no type, which the browser would press first', () => {
    const chips = form('<button>Chip</button><button type="submit">Add</button>');
    // What the browser picks for a Return in a field: the first button that submits.
    const browserDefault = [...chips.elements].find(
      (el) => (el as HTMLButtonElement).type === 'submit'
    );
    expect(browserDefault?.textContent).toBe('Chip');

    expect(findEnabledSubmitButton(chips)?.textContent).toBe('Add');
  });

  it('is null, not an untyped button, while the explicit submit button is disabled', () => {
    const found = findEnabledSubmitButton(
      form('<button>Chip</button><button type="submit" disabled>Add</button>')
    );
    expect(found).toBeNull();
  });

  it('is null when no button in the form says type="submit"', () => {
    expect(
      findEnabledSubmitButton(form('<button>Chip</button><button type="button">Cancel</button>'))
    ).toBeNull();
  });

  it('finds an input written as a submit button', () => {
    const found = findEnabledSubmitButton(form('<input /><input type="submit" value="Move" />'));
    expect((found as HTMLInputElement | null)?.value).toBe('Move');
  });

  it('is null when the first submit button is disabled, as for the browser', () => {
    const found = findEnabledSubmitButton(
      form('<button type="submit" disabled>Move</button><button type="submit">Copy</button>')
    );
    expect(found).toBeNull();
  });
});
