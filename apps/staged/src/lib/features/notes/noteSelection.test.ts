// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { headingPathFor, quoteFromSelection, rangeStartNode } from './noteSelection';

let root: HTMLElement;

function render(html: string) {
  root = document.createElement('div');
  root.innerHTML = html;
  document.body.replaceChildren(root);
}

function el(selector: string): Element {
  const found = root.querySelector(selector);
  if (!found) throw new Error(`no ${selector}`);
  return found;
}

function textOf(selector: string): Text {
  const node = el(selector).firstChild;
  if (!(node instanceof Text)) throw new Error(`${selector} has no text`);
  return node;
}

function select(start: Node, startOffset: number, end: Node, endOffset: number): Selection {
  const selection = document.getSelection()!;
  const range = document.createRange();
  range.setStart(start, startOffset);
  range.setEnd(end, endOffset);
  selection.removeAllRanges();
  selection.addRange(range);
  return selection;
}

const NOTE = `
  <h1 id="title">Rollout plan</h1>
  <p id="intro">Intro text.</p>
  <h2 id="arch">Architecture</h2>
  <p id="arch-p">Overview.</p>
  <h3 id="flow">Data  flow</h3>
  <p id="flow-p">The cache is invalidated on every write.</p>
  <h3 id="storage">Storage</h3>
  <ul><li id="storage-li">Sqlite, one file.</li></ul>
  <h2 id="risks">Risks</h2>
  <p id="risks-p">Migration.</p>
`;

describe('headingPathFor', () => {
  beforeEach(() => render(NOTE));

  it('walks the hierarchy down to the nearest heading', () => {
    expect(headingPathFor(root, textOf('#flow-p'))).toEqual([
      'Rollout plan',
      'Architecture',
      'Data flow',
    ]);
  });

  it('closes a section at the next heading of the same or a higher level', () => {
    expect(headingPathFor(root, textOf('#storage-li'))).toEqual([
      'Rollout plan',
      'Architecture',
      'Storage',
    ]);
    expect(headingPathFor(root, textOf('#risks-p'))).toEqual(['Rollout plan', 'Risks']);
  });

  it('puts a node inside a heading under that heading', () => {
    expect(headingPathFor(root, textOf('#flow'))).toEqual([
      'Rollout plan',
      'Architecture',
      'Data flow',
    ]);
  });

  it('leaves out ignored headings, such as the title', () => {
    const title = el('#title');

    expect(headingPathFor(root, textOf('#flow-p'), { ignore: (h) => h === title })).toEqual([
      'Architecture',
      'Data flow',
    ]);
    expect(headingPathFor(root, textOf('#intro'), { ignore: (h) => h === title })).toEqual([]);
  });

  it('is empty before the first heading', () => {
    render('<p id="p">No headings yet.</p><h2>Later</h2>');

    expect(headingPathFor(root, textOf('#p'))).toEqual([]);
  });
});

describe('rangeStartNode', () => {
  it('resolves an element boundary to the child at the offset', () => {
    render(NOTE);
    const range = document.createRange();
    range.setStart(root, 0);
    range.setEnd(root, 0);
    expect(rangeStartNode(range)).toBe(root.childNodes[0]);

    range.setStart(root, root.childNodes.length);
    expect(rangeStartNode(range)).toBe(root.childNodes[root.childNodes.length - 1]);
  });
});

describe('quoteFromSelection', () => {
  beforeEach(() => render(NOTE));

  it('quotes the selected text with the path it starts under', () => {
    const text = textOf('#flow-p');
    const quote = quoteFromSelection(select(text, 4, text, 9), root);

    expect(quote).toMatchObject({
      quote: 'cache',
      headings: ['Rollout plan', 'Architecture', 'Data flow'],
    });
    expect(quote?.range.toString()).toBe('cache');
  });

  it('keeps the copied range after the selection goes', () => {
    const text = textOf('#flow-p');
    const quote = quoteFromSelection(select(text, 4, text, 9), root);
    document.getSelection()!.removeAllRanges();

    expect(quote?.range.toString()).toBe('cache');
  });

  it('ignores collapsed, whitespace-only and out-of-note selections', () => {
    const text = textOf('#flow-p');
    expect(quoteFromSelection(select(text, 3, text, 3), root)).toBeNull();
    expect(quoteFromSelection(select(text, 3, text, 4), root)).toBeNull();

    const outside = document.createElement('p');
    outside.textContent = 'Chat text';
    document.body.appendChild(outside);
    expect(quoteFromSelection(select(outside.firstChild!, 0, text, 3), root)).toBeNull();
  });

  it('is null without a selection', () => {
    expect(quoteFromSelection(null, root)).toBeNull();
  });
});
