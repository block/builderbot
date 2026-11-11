// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { adjacentEditor, caretLineEdges, isPlainVerticalArrow } from './composerCaretNav';

// jsdom lays nothing out, so a caret's rect can't be measured here: the
// cases that need one report "can't tell" (null) and the field leaves the key
// to the input. What can be covered is the empty input, which has one line
// whatever the layout, the guards around the selection, and the walk through
// the inputs.

const editors: HTMLElement[] = [];

function editor(text = ''): HTMLElement {
  const el = document.createElement('div');
  el.contentEditable = 'true';
  el.textContent = text;
  document.body.appendChild(el);
  editors.push(el);
  return el;
}

function placeCaret(el: HTMLElement, offset = 0) {
  const range = document.createRange();
  const node = el.firstChild ?? el;
  range.setStart(node, offset);
  range.collapse(true);
  const selection = window.getSelection();
  if (!selection) throw new Error('no selection');
  selection.removeAllRanges();
  selection.addRange(range);
  return range;
}

afterEach(() => {
  for (const el of editors) el.remove();
  editors.length = 0;
  window.getSelection()?.removeAllRanges();
});

describe('caretLineEdges', () => {
  it('puts the caret of an empty input on both its first and last line', () => {
    const el = editor();
    placeCaret(el);

    expect(caretLineEdges(el)).toEqual({ first: true, last: true });
  });

  it('reports nothing without a collapsed caret in the input', () => {
    const el = editor('some text');
    const other = editor('elsewhere');

    expect(caretLineEdges(el)).toBeNull();

    placeCaret(other);
    expect(caretLineEdges(el)).toBeNull();

    const range = placeCaret(el, 1);
    range.setEnd(el.firstChild as Text, 4);
    expect(caretLineEdges(el)).toBeNull();
  });

  it('leaves the line alone when the caret has no measurable rect', () => {
    const el = editor('some text');
    placeCaret(el, 2);

    expect(caretLineEdges(el)).toBeNull();
  });
});

describe('adjacentEditor', () => {
  it('walks the inputs in order and stops at the ends', () => {
    const [a, b, c] = [editor(), editor(), editor()];

    expect(adjacentEditor([a, b, c], b, 'up')).toBe(a);
    expect(adjacentEditor([a, b, c], b, 'down')).toBe(c);
    expect(adjacentEditor([a, b, c], a, 'up')).toBeNull();
    expect(adjacentEditor([a, b, c], c, 'down')).toBeNull();
    expect(adjacentEditor([a, b], c, 'down')).toBeNull();
  });
});

describe('isPlainVerticalArrow', () => {
  const key = (init: Partial<KeyboardEvent>) =>
    isPlainVerticalArrow({
      key: 'ArrowUp',
      shiftKey: false,
      altKey: false,
      ctrlKey: false,
      metaKey: false,
      ...init,
    });

  it('takes an unmodified ArrowUp or ArrowDown only', () => {
    expect(key({})).toBe(true);
    expect(key({ key: 'ArrowDown' })).toBe(true);
    expect(key({ key: 'ArrowLeft' })).toBe(false);
    expect(key({ shiftKey: true })).toBe(false);
    expect(key({ altKey: true })).toBe(false);
    expect(key({ ctrlKey: true })).toBe(false);
    expect(key({ metaKey: true })).toBe(false);
  });
});
