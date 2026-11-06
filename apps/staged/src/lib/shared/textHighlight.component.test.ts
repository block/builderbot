import { beforeEach, describe, expect, it } from 'vitest';
import { clearHighlights, highlightMatches } from './textHighlight';

// jsdom is only provided to `*.component.test.ts` files (see vitest.config.ts).

let container: HTMLElement;

beforeEach(() => {
  document.body.innerHTML = '';
  container = document.createElement('div');
  document.body.appendChild(container);
});

/** A container holding one text node per entry, in order. */
function fill(...texts: string[]): Text[] {
  const nodes = texts.map((text) => document.createTextNode(text));
  container.append(...nodes);
  return nodes;
}

const textNodes = () => [...container.childNodes].map((n) => (n as Text).data);
const marks = () => container.querySelectorAll('mark.search-match');

describe('clearHighlights', () => {
  describe.each([
    ['a match at the start', 'foo bar baz', 'foo'],
    ['a match in the middle', 'foo bar baz', 'bar'],
    ['a match at the end', 'foo bar baz', 'baz'],
    ['a match covering the whole node', 'foo', 'foo'],
    ['several matches in one node', 'ab ab ab', 'ab'],
    ['matches differing only in case', 'Foo foo FOO', 'foo'],
  ])('after highlighting %s', (_label, text, query) => {
    it('restores the text as a single text node', () => {
      fill(text);
      const { total } = highlightMatches(container, query, 0);
      expect(total).toBeGreaterThan(0);

      clearHighlights(container);

      expect(marks()).toHaveLength(0);
      expect(textNodes()).toEqual([text]);
    });
  });

  it('merges adjacent matches', () => {
    fill('aaa');
    expect(highlightMatches(container, 'a', 0).total).toBe(3);

    clearHighlights(container);

    expect(textNodes()).toEqual(['aaa']);
  });

  it('merges adjacent matches with text around them', () => {
    fill('xaaay');
    expect(highlightMatches(container, 'a', 0).total).toBe(3);

    clearHighlights(container);

    expect(textNodes()).toEqual(['xaaay']);
  });

  it('restores the text of several text nodes', () => {
    // Highlighting replaces each matching node with fresh pieces, and a piece
    // that sits next to another node's text may be merged with it (as
    // `normalize()` did), so only the text is asserted here.
    fill('one two', 'two three', 'four');
    expect(highlightMatches(container, 'two', 0).total).toBe(2);

    clearHighlights(container);

    expect(marks()).toHaveLength(0);
    expect(container.textContent).toBe('one twotwo threefour');
  });

  it('restores matches nested in elements', () => {
    container.innerHTML = '<p>a <b>bold</b> word</p><p>bold</p>';
    expect(highlightMatches(container, 'bold', 0).total).toBe(2);

    clearHighlights(container);

    expect(container.innerHTML).toBe('<p>a <b>bold</b> word</p><p>bold</p>');
    expect([...container.querySelectorAll('b')].map((b) => b.childNodes.length)).toEqual([1]);
  });

  it('leaves empty text nodes alone, which Svelte uses as block anchors', () => {
    const before = document.createTextNode('');
    const [text] = fill('foo bar baz');
    const between = document.createTextNode('');
    const after = document.createTextNode('');
    container.insertBefore(before, text);
    container.append(between, document.createTextNode('bar'), after);

    expect(highlightMatches(container, 'bar', 0).total).toBe(2);
    clearHighlights(container);

    expect(textNodes()).toEqual(['', 'foo bar baz', '', 'bar', '']);
    expect(container.childNodes[0]).toBe(before);
    expect(container.childNodes[2]).toBe(between);
    expect(container.childNodes[4]).toBe(after);
  });

  it('keeps an empty text node after the text a match was merged into', () => {
    const [text] = fill('foo bar');
    const anchor = document.createTextNode('');
    text.after(anchor);

    highlightMatches(container, 'foo', 0);
    clearHighlights(container);

    expect(textNodes()).toEqual(['foo bar', '']);
    expect(container.lastChild).toBe(anchor);
  });

  it('keeps empty text nodes beside matches that filled a whole node', () => {
    const before = document.createTextNode('');
    const [text] = fill('foo');
    const after = document.createTextNode('');
    text.before(before);
    text.after(after);

    highlightMatches(container, 'foo', 0);
    clearHighlights(container);

    expect(textNodes()).toEqual(['', 'foo', '']);
    expect(container.firstChild).toBe(before);
    expect(container.lastChild).toBe(after);
  });

  it('leaves the DOM untouched when nothing is highlighted', () => {
    container.append(document.createTextNode(''), document.createElement('span'));
    container.append(document.createTextNode('foo'), document.createTextNode('bar'));
    const children = [...container.childNodes];

    clearHighlights(container);

    expect([...container.childNodes]).toEqual(children);
    children.forEach((child, i) => expect(container.childNodes[i]).toBe(child));
    // Adjacent text nodes are not merged either.
    expect(textNodes().slice(2)).toEqual(['foo', 'bar']);
  });

  it('is safe to call twice', () => {
    fill('foo bar');
    highlightMatches(container, 'bar', 0);
    clearHighlights(container);
    clearHighlights(container);

    expect(textNodes()).toEqual(['foo bar']);
  });
});

describe('searching again on the same container', () => {
  // NoteModal's `performSearch` clears, then highlights again, per keystroke.
  it('re-highlights the merged text for a different query, then clears it', () => {
    fill('The quick brown fox', 'jumps over the lazy dog');
    const originalText = container.textContent;

    expect(highlightMatches(container, 'the', 0).total).toBe(2);
    clearHighlights(container);
    expect(textNodes()).toEqual(['The quick brown fox', 'jumps over the lazy dog']);

    const second = highlightMatches(container, 'o', 1);
    expect(second.total).toBe(4);
    expect(second.elements.map((el) => el.textContent)).toEqual(['o', 'o', 'o', 'o']);
    expect(second.elements[1].classList.contains('search-match-current')).toBe(true);
    expect(container.textContent).toBe(originalText);

    clearHighlights(container);
    expect(marks()).toHaveLength(0);
    expect(textNodes()).toEqual(['The quick brown fox', 'jumps over the lazy dog']);
    expect(container.textContent).toBe(originalText);
  });

  it('finds a match spanning what used to be a mark boundary once cleared', () => {
    fill('abcabc');
    expect(highlightMatches(container, 'b', 0).total).toBe(2);
    clearHighlights(container);

    // 'ca' straddles where the first search split the node.
    const { total } = highlightMatches(container, 'ca', 0);
    expect(total).toBe(1);

    clearHighlights(container);
    expect(textNodes()).toEqual(['abcabc']);
  });

  it('refines a query character by character', () => {
    fill('search the searchable search');
    for (const query of ['s', 'se', 'sea', 'sear', 'search']) {
      clearHighlights(container);
      highlightMatches(container, query, 0);
    }
    expect(marks()).toHaveLength(3);

    clearHighlights(container);
    expect(textNodes()).toEqual(['search the searchable search']);
  });
});
