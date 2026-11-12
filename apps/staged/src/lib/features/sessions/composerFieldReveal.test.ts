// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { revealElementInField, revealInField } from './composerFieldReveal';

// jsdom lays nothing out, so the field and its input are given boxes: the
// field's view is the 100px from 201 to 301 (a 1px border above), and its
// scroll offset is kept on the element. What is covered is the least scroll
// that brings an input into view, an input already in view left alone, and a
// tall input followed by its caret's line rather than its top. A block
// nobody is typing in (one Undo put back) is shown whole, or by its top
// when it is taller than the view.

const originalGetClientRects = Object.getOwnPropertyDescriptor(Range.prototype, 'getClientRects');
const made: HTMLElement[] = [];

function rect(top: number, bottom: number): DOMRect {
  return { top, bottom, height: bottom - top, left: 0, right: 10, width: 10 } as DOMRect;
}

function field(scrollTop = 0): HTMLElement {
  const el = document.createElement('div');
  Object.defineProperty(el, 'clientHeight', { value: 100 });
  Object.defineProperty(el, 'clientTop', { value: 1 });
  let scroll = scrollTop;
  Object.defineProperty(el, 'scrollTop', {
    get: () => scroll,
    set: (value: number) => {
      scroll = value;
    },
  });
  el.getBoundingClientRect = () => rect(200, 302);
  document.body.appendChild(el);
  made.push(el);
  return el;
}

function input(parent: HTMLElement, top: number, bottom: number, text = ''): HTMLElement {
  const el = document.createElement('div');
  el.contentEditable = 'true';
  el.textContent = text;
  el.getBoundingClientRect = () => rect(top, bottom);
  parent.appendChild(el);
  return el;
}

afterEach(() => {
  for (const el of made) el.remove();
  made.length = 0;
  window.getSelection()?.removeAllRanges();
  if (originalGetClientRects) {
    Object.defineProperty(Range.prototype, 'getClientRects', originalGetClientRects);
  } else {
    delete (Range.prototype as Partial<Range>).getClientRects;
  }
});

describe('revealInField', () => {
  it('scrolls the field down by the least that shows an input below its view', () => {
    const el = field();
    const target = input(el, 320, 350);

    revealInField(el, target);

    expect(el.scrollTop).toBe(49);
  });

  it('scrolls the field up by the least that shows an input above its view', () => {
    const el = field(80);
    const target = input(el, 150, 180);

    revealInField(el, target);

    expect(el.scrollTop).toBe(29);
  });

  it('leaves the field alone for an input already in view', () => {
    const el = field(40);
    const target = input(el, 210, 290);

    revealInField(el, target);

    expect(el.scrollTop).toBe(40);
  });

  it('follows the caret’s line in an input taller than the view', () => {
    const el = field();
    const target = input(el, 150, 400, 'abcdefghij');
    const text = target.firstChild as Text;
    // A character on the line from 330 to 350; a collapsed range has no rect.
    Object.defineProperty(Range.prototype, 'getClientRects', {
      configurable: true,
      writable: true,
      value(this: Range) {
        return this.collapsed || this.startContainer !== text ? [] : [rect(330, 350)];
      },
    });
    const range = document.createRange();
    range.setStart(text, 4);
    range.collapse(true);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);

    revealInField(el, target);
    expect(el.scrollTop).toBe(49);

    // With no caret to follow, the field stays.
    window.getSelection()?.removeAllRanges();
    revealInField(el, target);
    expect(el.scrollTop).toBe(49);
  });
});

describe('revealElementInField', () => {
  it('scrolls the field by the least that shows a block below its view', () => {
    const el = field();
    const block = input(el, 320, 380);

    revealElementInField(el, block);

    expect(el.scrollTop).toBe(79);
  });

  it('shows the top of a block taller than the view, having no caret to follow', () => {
    const el = field(40);
    const block = input(el, 320, 600);

    revealElementInField(el, block);

    // The block's top is now at the view's top, 201.
    expect(el.scrollTop).toBe(159);
  });

  it('leaves the field alone for a block already in view', () => {
    const el = field(40);
    const block = input(el, 210, 290);

    revealElementInField(el, block);

    expect(el.scrollTop).toBe(40);
  });
});
