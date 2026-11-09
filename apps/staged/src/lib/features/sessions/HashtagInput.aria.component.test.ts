import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { HashtagItem } from '../../types';
import HashtagInput from './HashtagInput.svelte';

// The editor's ARIA wiring: a multi-line textbox (not a combobox, which would
// announce the message box as a single-line field) that exposes the # dropdown
// through aria-haspopup/aria-controls/aria-activedescendant, named by its
// placeholder, with the empty result announced as a status.

const noteItem: HashtagItem = {
  type: 'note',
  id: 'n1',
  title: 'Mobile audit notes',
  color: '#000',
  bgColor: '#fff',
};

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

function render(props: { placeholder?: string; items?: HashtagItem[] } = {}) {
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(HashtagInput, {
    target,
    props: { value: '', items: props.items ?? [], placeholder: props.placeholder },
  });
  flushSync();
  const editor = target.querySelector<HTMLElement>('.hashtag-editor');
  if (!editor) throw new Error('editor not rendered');
  return editor;
}

/** Type `text` into the editor with the caret at its end, as a keystroke would leave it. */
function type(editor: HTMLElement, text: string) {
  editor.textContent = text;
  const textNode = editor.firstChild;
  if (!textNode) throw new Error('no text node');
  const range = document.createRange();
  range.setStart(textNode, text.length);
  range.collapse(true);
  const selection = window.getSelection();
  if (!selection) throw new Error('no selection');
  selection.removeAllRanges();
  selection.addRange(range);
  editor.dispatchEvent(new InputEvent('input', { bubbles: true }));
  flushSync();
}

beforeEach(() => {
  // jsdom lays nothing out; the dropdown placement only needs a rect.
  if (!('getBoundingClientRect' in Range.prototype)) {
    Object.defineProperty(Range.prototype, 'getBoundingClientRect', {
      configurable: true,
      value: () => ({ top: 0, bottom: 0, left: 0, right: 0, width: 0, height: 0 }),
    });
  }
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  target.remove();
  document.querySelectorAll('.hashtag-dropdown').forEach((el) => el.remove());
});

describe('HashtagInput ARIA', () => {
  it('is a multi-line textbox named by its placeholder, with a listbox popup', () => {
    const editor = render({ placeholder: 'Message…' });
    expect(editor.getAttribute('role')).toBe('textbox');
    expect(editor.getAttribute('aria-multiline')).toBe('true');
    expect(editor.getAttribute('aria-label')).toBe('Message…');
    expect(editor.getAttribute('aria-haspopup')).toBe('listbox');
    expect(editor.getAttribute('aria-autocomplete')).toBe('list');
    // Combobox-only; a textbox must not carry it.
    expect(editor.hasAttribute('aria-expanded')).toBe(false);
    // Nothing to point at while the dropdown is closed.
    expect(editor.hasAttribute('aria-controls')).toBe(false);
    expect(editor.hasAttribute('aria-activedescendant')).toBe(false);
  });

  it('has no aria-label without a placeholder', () => {
    const editor = render();
    expect(editor.hasAttribute('aria-label')).toBe(false);
  });

  it('points at the open listbox and its highlighted option', () => {
    const editor = render({ placeholder: 'Message…', items: [noteItem] });
    type(editor, '#mob');

    const listbox = document.querySelector<HTMLElement>('[role="listbox"]');
    expect(listbox).not.toBeNull();
    expect(editor.getAttribute('aria-controls')).toBe(listbox!.id);
    const option = listbox!.querySelector<HTMLElement>('[role="option"]');
    expect(option?.getAttribute('aria-selected')).toBe('true');
    expect(editor.getAttribute('aria-activedescendant')).toBe(option!.id);
    expect(editor.getAttribute('role')).toBe('textbox');
    expect(editor.hasAttribute('aria-expanded')).toBe(false);
  });

  it('announces an empty result as a status and drops the listbox references', () => {
    const editor = render({ placeholder: 'Message…', items: [noteItem] });
    type(editor, '#zzz');

    expect(document.querySelector('[role="listbox"]')).toBeNull();
    const status = document.querySelector<HTMLElement>('[role="status"]');
    expect(status?.textContent?.trim()).toBe('No matching items');
    expect(editor.hasAttribute('aria-controls')).toBe(false);
    expect(editor.hasAttribute('aria-activedescendant')).toBe(false);
  });
});
