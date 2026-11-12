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
// caret, the caret at a soft wrap, around line breaks and around the
// `<div>` lines a browser's Enter makes, the ends known without a layout,
// the guards around the selection, and the walk through the inputs.

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

  it('reads a caret at a soft wrap as at the start of the lower line, where a browser draws it', () => {
    const el = wrapped('abcdefghijklmnopqrstuvwxyz1234', 10);
    const text = el.firstChild as Text;

    // Between the first and second line: on the second, so ArrowUp from
    // where ArrowDown out of the first line landed goes back up a line
    // rather than out of the input.
    placeCaret(text, 10);
    expect(caretLineEdges(el)).toEqual({ first: false, last: false });

    // Between the second and the last: on the last, so the first ArrowDown
    // crosses rather than the browser moving to the end.
    placeCaret(text, 20);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
  });

  it('takes the caret’s own rect only where it lies on one line, and measures the neighbours at a wrap', () => {
    // Chromium gives a collapsed range in a text node rects where WebKit
    // gives none. Mid-line there is one, on the caret's line. At a soft
    // wrap there are two — the end of the upper line and the start of the
    // lower — whichever line the caret is drawn on (probed: the same pair
    // after End, after Home and after a collapse), so they say nothing
    // about the caret's line and the neighbours are read as in WebKit.
    // Taking the first of the pair put every caret at a wrap on the upper
    // line, and ArrowUp from the start of the second line left the input.
    const el = wrapped('abcdefghijklmnopqrst', 10);
    const text = el.firstChild as Text;
    const neighbours = Object.getOwnPropertyDescriptor(Range.prototype, 'getClientRects')!
      .value as (this: Range) => DOMRect[];
    let neighbourReads = 0;
    Object.defineProperty(Range.prototype, 'getClientRects', {
      configurable: true,
      writable: true,
      value(this: Range) {
        if (!this.collapsed) {
          neighbourReads += 1;
          return neighbours.call(this);
        }
        if (this.startContainer !== text) return [];
        // At the wrap, a rect on each line; mid-line, the caret's own line;
        // elsewhere a rect with no height, as good as none.
        if (this.startOffset === 10) return [lineRect(0), lineRect(1)];
        if (this.startOffset === 15) return [lineRect(1)];
        return [rect(0, 0, 0)];
      },
    });

    // At the wrap the caret counts as on the lower line, read from the
    // character after it: ArrowUp from there moves up a line in the input.
    placeCaret(text, 10);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
    expect(caretLineRectIn(el)).toEqual({
      top: LINE_TOP + LINE_HEIGHT,
      bottom: LINE_TOP + 2 * LINE_HEIGHT,
    });
    expect(neighbourReads).toBeGreaterThan(0);

    // Mid-line the caret's own rect is taken, and nothing else is measured.
    neighbourReads = 0;
    placeCaret(text, 15);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
    expect(caretLineRectIn(el)).toEqual({
      top: LINE_TOP + LINE_HEIGHT,
      bottom: LINE_TOP + 2 * LINE_HEIGHT,
    });
    expect(neighbourReads).toBe(0);

    // Without a usable rect of its own the caret is measured from its
    // neighbours: the character after, on the upper line here.
    placeCaret(text, 5);
    expect(caretLineEdges(el)).toEqual({ first: true, last: false });
    expect(caretLineRectIn(el)).toEqual({ top: LINE_TOP, bottom: LINE_TOP + LINE_HEIGHT });
    expect(neighbourReads).toBeGreaterThan(0);
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

  it('treats the block a browser wraps the next line in as a line boundary', () => {
    // `line1<div>line2</div>`: Enter in Chromium and WebKit, where the
    // extractor reads a line break; nothing re-renders it from the value.
    const el = editor();
    const a = document.createTextNode('abc');
    const div = document.createElement('div');
    const b = document.createTextNode('def');
    div.append(b);
    el.append(a, div);
    layOut(el, 2);
    div.getBoundingClientRect = () => lineRect(1);
    stubRangeRects((node) => (node === a ? 0 : node === b ? 1 : null));

    // The end of the first line: the block after it says nothing about the
    // caret's line, so the character before is read, and ArrowDown moves
    // onto the second line rather than out of the input.
    placeCaret(a, 3);
    expect(caretLineEdges(el)).toEqual({ first: true, last: false });
    placeCaret(el, 1);
    expect(caretLineEdges(el)).toEqual({ first: true, last: false });

    // Inside the block.
    placeCaret(b, 0);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
    placeCaret(b, 3);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });

    // After the block, with nothing following: its last line.
    placeCaret(el, 2);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
  });

  it('puts a caret after a block with more below it on the line below the block', () => {
    // `<div>abc</div><div>def</div>` with the caret between the blocks, and
    // an empty line (`<div><br></div>`) at the end, as Enter twice leaves.
    const el = editor();
    const first = document.createElement('div');
    first.append('abc');
    const second = document.createElement('div');
    second.append('def');
    const empty = document.createElement('div');
    const br = document.createElement('br');
    empty.append(br);
    el.append(first, second, empty);
    layOut(el, 3);
    first.getBoundingClientRect = () => lineRect(0);
    second.getBoundingClientRect = () => lineRect(1);
    empty.getBoundingClientRect = () => lineRect(2);
    br.getBoundingClientRect = () =>
      rect(LINE_TOP + 2 * LINE_HEIGHT, LINE_TOP + 3 * LINE_HEIGHT, 0);
    stubRangeRects((node) =>
      node === first.firstChild ? 0 : node === second.firstChild ? 1 : null
    );

    placeCaret(el, 1);
    expect(caretLineEdges(el)).toEqual({ first: false, last: false });

    // The empty last line, by its break.
    placeCaret(empty, 0);
    expect(caretLineEdges(el)).toEqual({ first: false, last: true });
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
