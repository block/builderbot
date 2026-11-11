// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import {
  adjacentEditor,
  caretLineEdges,
  caretLineRectIn,
  isPlainVerticalArrow,
} from './composerCaretNav';

// jsdom lays nothing out, so the layout is stood in: an input is given a box
// of so many lines, and ranges report rects as WebKit does — none for a
// collapsed range, one for a range over a character, on that character's
// line. The cases cover the measurement from the characters next to the
// caret, the caret at a soft wrap and around line breaks, the ends known
// without a layout, the guards around the selection, and the walk through
// the inputs.

const LINE_HEIGHT = 20;
const LINE_TOP = 100;
const PADDING = 4;

const editors: HTMLElement[] = [];

function editor(text = ''): HTMLElement {
  const el = document.createElement('div');
  el.contentEditable = 'true';
  el.textContent = text;
  document.body.appendChild(el);
  editors.push(el);
  return el;
}

/** Lay `el` out as `lines` lines of text, padded top and bottom. */
function layOut(el: HTMLElement, lines: number) {
  el.style.paddingTop = `${PADDING}px`;
  el.style.paddingBottom = `${PADDING}px`;
  el.style.lineHeight = `${LINE_HEIGHT}px`;
  const top = LINE_TOP - PADDING;
  const bottom = LINE_TOP + lines * LINE_HEIGHT + PADDING;
  el.getBoundingClientRect = () => rect(top, bottom);
}

function rect(top: number, bottom: number, width = 7): DOMRect {
  return {
    top,
    bottom,
    height: bottom - top,
    left: 10,
    right: 10 + width,
    width,
    x: 10,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

function lineRect(line: number): DOMRect {
  return rect(LINE_TOP + line * LINE_HEIGHT, LINE_TOP + (line + 1) * LINE_HEIGHT);
}

const originalGetClientRects = Object.getOwnPropertyDescriptor(Range.prototype, 'getClientRects');

/**
 * Give ranges rects as WebKit does: none when collapsed, otherwise the rect
 * of the line `lineOf` puts the range's start on (`null` for no rect).
 */
function stubRangeRects(lineOf: (node: Node, offset: number) => number | null) {
  Object.defineProperty(Range.prototype, 'getClientRects', {
    configurable: true,
    writable: true,
    value(this: Range) {
      if (this.collapsed) return [];
      const line = lineOf(this.startContainer, this.startOffset);
      return line === null ? [] : [lineRect(line)];
    },
  });
}

/** A one-text-node input wrapped every `charsPerLine` characters. */
function wrapped(text: string, charsPerLine: number): HTMLElement {
  const el = editor(text);
  layOut(el, Math.ceil(text.length / charsPerLine));
  stubRangeRects((node, offset) =>
    node === el.firstChild ? Math.floor(offset / charsPerLine) : null
  );
  return el;
}

function placeCaret(node: Node, offset = 0) {
  const range = document.createRange();
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
  if (originalGetClientRects) {
    Object.defineProperty(Range.prototype, 'getClientRects', originalGetClientRects);
  } else {
    delete (Range.prototype as Partial<Range>).getClientRects;
  }
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

    placeCaret(other.firstChild as Text);
    expect(caretLineEdges(el)).toBeNull();

    const range = placeCaret(el.firstChild as Text, 1);
    range.setEnd(el.firstChild as Text, 4);
    expect(caretLineEdges(el)).toBeNull();
  });

  it('measures the line from the characters next to the caret, which a collapsed range cannot give', () => {
    const el = wrapped('abcdefghijklmnopqrstuvwxyz1234', 10);
    const text = el.firstChild as Text;

    placeCaret(text, 3);
    expect(caretLineEdges(el)).toEqual({ first: true, last: false });

    placeCaret(text, 15);
    expect(caretLineEdges(el)).toEqual({ first: false, last: false });

    placeCaret(text, 27);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
  });

  it('reads a caret at a soft wrap as on the line in the arrow’s direction', () => {
    const el = wrapped('abcdefghijklmnopqrstuvwxyz1234', 10);
    const text = el.firstChild as Text;

    // Between the first and second line: up counts it on the first.
    placeCaret(text, 10);
    expect(caretLineEdges(el)).toEqual({ first: true, last: false });

    // Between the second and the last: down counts it on the last, so the
    // first ArrowDown crosses rather than the browser moving to the end.
    placeCaret(text, 20);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
  });

  it('knows a caret at either end of the content without measuring that side', () => {
    const el = wrapped('abcdefghijklmnopqrst', 10);
    const text = el.firstChild as Text;

    placeCaret(text, 0);
    expect(caretLineEdges(el)).toEqual({ first: true, last: false });

    placeCaret(text, 20);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
  });

  it('puts a caret after a line break on the line below it, and one before it on the line above', () => {
    const el = editor();
    const a = document.createTextNode('a');
    const br = document.createElement('br');
    const b = document.createTextNode('b');
    el.append(a, br, b);
    layOut(el, 2);
    br.getBoundingClientRect = () => rect(LINE_TOP, LINE_TOP + LINE_HEIGHT, 0);
    stubRangeRects((node) => (node === a ? 0 : node === b ? 1 : null));

    placeCaret(b, 0);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
    placeCaret(el, 2);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });

    placeCaret(a, 1);
    expect(caretLineEdges(el)).toEqual({ first: true, last: false });
    placeCaret(el, 1);
    expect(caretLineEdges(el)).toEqual({ first: true, last: false });
  });

  it('counts a trailing line break as part of the line it ends', () => {
    const el = editor();
    const a = document.createTextNode('a');
    const br = document.createElement('br');
    el.append(a, br);
    layOut(el, 1);
    br.getBoundingClientRect = () => rect(LINE_TOP, LINE_TOP + LINE_HEIGHT, 0);
    stubRangeRects((node) => (node === a ? 0 : null));

    placeCaret(el, 2);
    expect(caretLineEdges(el)).toEqual({ first: true, last: true });
    placeCaret(a, 1);
    expect(caretLineEdges(el)).toEqual({ first: true, last: true });
  });

  it('leaves the line alone when nothing by the caret can be measured', () => {
    const el = editor('some text');
    placeCaret(el.firstChild as Text, 2);
    expect(caretLineEdges(el)).toBeNull();

    layOut(el, 1);
    stubRangeRects(() => null);
    expect(caretLineEdges(el)).toBeNull();
  });
});

describe('caretLineRectIn', () => {
  it('gives the line the caret’s next character starts, and nothing without a caret', () => {
    const el = wrapped('abcdefghijklmnopqrst', 10);
    const text = el.firstChild as Text;

    placeCaret(text, 10);
    expect(caretLineRectIn(el)).toEqual({
      top: LINE_TOP + LINE_HEIGHT,
      bottom: LINE_TOP + 2 * LINE_HEIGHT,
    });

    placeCaret(text, 20);
    expect(caretLineRectIn(el)).toEqual({
      top: LINE_TOP + LINE_HEIGHT,
      bottom: LINE_TOP + 2 * LINE_HEIGHT,
    });

    window.getSelection()?.removeAllRanges();
    expect(caretLineRectIn(el)).toBeNull();
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
