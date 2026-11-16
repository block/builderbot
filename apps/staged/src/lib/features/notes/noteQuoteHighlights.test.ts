// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { clearHighlights, highlightMatches } from '../../shared/textHighlight';
import {
  applyQuoteHighlights,
  clearQuoteHighlights,
  highlightNoteMatches,
  quoteHighlightElements,
  quoteHighlightRange,
  quoteHighlightStop,
  quoteIdAtTarget,
  setQuoteHighlightState,
  type HighlightableQuote,
} from './noteQuoteHighlights';
import { captureQuoteAnchor } from './noteQuoteLocation';

// The passages a reply draft quotes are wrapped in highlight spans, a text
// node at a time, and taken out again without a trace.

let root: HTMLElement;

function render(html: string) {
  root = document.createElement('div');
  root.innerHTML = html;
  document.body.replaceChildren(root);
}

const NOTE = `
  <h2>Architecture</h2>
  <p id="flow">The cache is <strong>invalidated</strong> on <em>every</em> write.</p>
  <ul>
    <li>Reads go to the cache first.</li>
    <li>Writes go to the <a href="https://example.com">primary store</a> only.</li>
  </ul>
  <h2>Rollout</h2>
  <p id="ship">Ship behind a flag. Reads go to the cache first.</p>
  <pre><code>cache.invalidate();
cache.warm();</code></pre>
  <table>
    <tr><th>Stage</th><th>Owner</th></tr>
    <tr><td>Canary</td><td>Infra</td></tr>
  </table>
  <figure class="markdown-diagram" role="button" tabindex="0"><svg><text>Cache box</text></svg></figure>
  <p id="after">After the diagram.</p>
`;

function quote(id: string, text: string, extra: Partial<HighlightableQuote> = {}) {
  return { id, quote: text, headings: [], ...extra };
}

function marks(): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>('.note-quote-highlight')];
}

/** The marked text of a quote, a piece per span. */
function marked(id: string): string[] {
  return quoteHighlightElements(root, id).map((span) => span.textContent ?? '');
}

beforeEach(() => render(NOTE));

describe('applyQuoteHighlights', () => {
  it('wraps a passage through inline elements without moving any of them', () => {
    const ids = applyQuoteHighlights(root, [quote('q1', 'cache is invalidated on every')]);

    expect(ids).toEqual(['q1']);
    expect(marked('q1')).toEqual(['cache is ', 'invalidated', ' on ', 'every']);
    // The bold and italic elements are where they were, with a span inside.
    expect(root.querySelector('#flow > strong > .note-quote-highlight')).not.toBeNull();
    expect(root.querySelector('#flow > em > .note-quote-highlight')).not.toBeNull();
    expect(root.querySelector('#flow')!.textContent).toBe(
      'The cache is invalidated on every write.'
    );
  });

  it('wraps a passage across blocks and list items, leaving the markup between alone', () => {
    applyQuoteHighlights(root, [
      quote('q1', 'every write.\n\nReads go to the cache first.\nWrites go'),
    ]);

    expect(marked('q1')).toEqual(['every', ' write.', 'Reads go to the cache first.', 'Writes go']);
    // No span sits between blocks: a list holds only its items.
    expect([...root.querySelector('ul')!.children].map((child) => child.tagName)).toEqual([
      'LI',
      'LI',
    ]);
    expect(marks().every((span) => span.textContent!.trim().length > 0)).toBe(true);
  });

  it('wraps passages in a code block and across table cells', () => {
    applyQuoteHighlights(root, [
      quote('code', 'invalidate();\ncache.warm'),
      quote('cells', 'Owner\nCanary\tInfra'),
    ]);

    expect(marked('code')).toEqual(['invalidate();\ncache.warm']);
    expect(marked('cells')).toEqual(['Owner', 'Canary', 'Infra']);
    // The table's rows still hold only cells.
    for (const row of root.querySelectorAll('tr')) {
      expect([...row.children].every((cell) => /^T[HD]$/.test(cell.tagName))).toBe(true);
    }
  });

  it('marks the occurrence the quote was selected at', () => {
    const text = root.querySelector('#ship')!.firstChild as Text;
    const start = text.data.indexOf('Reads');
    const range = document.createRange();
    range.setStart(text, start);
    range.setEnd(text, text.length);
    const anchor = captureQuoteAnchor(root, range, 'Reads go to the cache first.')!;

    applyQuoteHighlights(root, [quote('q1', 'Reads go to the cache first.', { anchor })]);

    expect(root.querySelector('li .note-quote-highlight')).toBeNull();
    expect(root.querySelector('#ship .note-quote-highlight')!.textContent).toBe(
      'Reads go to the cache first.'
    );
  });

  it('cuts overlapping quotes at each edge, and gives the shared words to the shorter', () => {
    applyQuoteHighlights(root, [
      quote('long', 'Ship behind a flag. Reads go to'),
      quote('short', 'a flag. Reads go to the cache'),
    ]);

    const spans = [...root.querySelectorAll<HTMLElement>('#ship .note-quote-highlight')];
    expect(
      spans.map((span) => [span.textContent, span.dataset.quoteIds, span.dataset.quoteId])
    ).toEqual([
      ['Ship behind ', 'long', 'long'],
      ['a flag. Reads go to', 'long short', 'short'],
      [' the cache', 'short', 'short'],
    ]);
    expect(spans.map((span) => span.classList.contains('is-overlap'))).toEqual([
      false,
      true,
      false,
    ]);
    // Both are whole, shared words and all.
    expect(marked('long').join('')).toBe('Ship behind a flag. Reads go to');
    expect(marked('short').join('')).toBe('a flag. Reads go to the cache');
  });

  it('keeps a quote inside a longer one reachable, and the newer of two on the same words', () => {
    applyQuoteHighlights(root, [
      quote('outer', 'Ship behind a flag.'),
      quote('inner', 'behind'),
      quote('twin', 'behind'),
    ]);

    const spans = [...root.querySelectorAll<HTMLElement>('#ship .note-quote-highlight')];
    expect(spans.map((span) => [span.textContent, span.dataset.quoteId])).toEqual([
      ['Ship ', 'outer'],
      ['behind', 'twin'],
      [' a flag.', 'outer'],
    ]);
    expect(marked('inner')).toEqual(['behind']);
  });

  it('marks adjacent quotes as separate spans', () => {
    applyQuoteHighlights(root, [quote('a', 'Ship behind'), quote('b', 'a flag.')]);

    const spans = [...root.querySelectorAll<HTMLElement>('#ship .note-quote-highlight')];
    expect(spans.map((span) => [span.textContent, span.dataset.quoteIds])).toEqual([
      ['Ship behind', 'a'],
      ['a flag.', 'b'],
    ]);
  });

  it("marks text in a link without taking the link's press", () => {
    applyQuoteHighlights(root, [quote('q1', 'go to the primary store only.')]);

    const spans = quoteHighlightElements(root, 'q1');
    expect(spans.map((span) => span.textContent)).toEqual([
      'go to the ',
      'primary store',
      ' only.',
    ]);
    const inLink = root.querySelector<HTMLElement>('a .note-quote-highlight')!;
    expect(inLink.dataset.quoteId).toBeUndefined();
    expect(inLink.title).toBe('');
    expect(quoteIdAtTarget(root, inLink)).toBeNull();
    expect(quoteIdAtTarget(root, spans[0])).toBe('q1');
    expect(quoteIdAtTarget(root, spans[0].firstChild)).toBe('q1');
    expect(quoteIdAtTarget(root, root.querySelector('#flow'))).toBeNull();
  });

  it('gives each quote one keyboard stop: its first span that can be pressed', () => {
    applyQuoteHighlights(root, [
      quote('q1', 'primary store only.'),
      quote('q2', 'cache is invalidated'),
      quote('q3', 'primary'),
    ]);

    const stops = [...root.querySelectorAll<HTMLElement>('.note-quote-highlight[role="button"]')];
    expect(stops.map((span) => [span.dataset.quoteId, span.textContent, span.tabIndex])).toEqual([
      ['q2', 'cache is ', 0],
      // Past the link, whose text is the link's to press.
      ['q1', ' only.', 0],
    ]);
    expect(quoteHighlightStop(root, 'q1')).toBe(stops[1]);
    // Wholly inside a link: marked, with no press or stop of its own.
    expect(marked('q3')).toEqual(['primary']);
    expect(quoteHighlightStop(root, 'q3')).toBeNull();
  });

  it('leaves a diagram alone while marking the text on both sides of it', () => {
    const figure = root.querySelector('figure')!.outerHTML;

    applyQuoteHighlights(root, [quote('q1', 'Infra\nCache box\nAfter the')]);

    expect(marked('q1')).toEqual(['Infra', 'After the']);
    expect(root.querySelector('figure')!.outerHTML).toBe(figure);
  });

  it('marks nothing for a quote the note no longer holds, and keeps the others', () => {
    const ids = applyQuoteHighlights(root, [
      quote('gone', 'This sentence was rewritten.'),
      quote('here', 'Ship behind a flag.'),
      quote('blank', '   '),
    ]);

    expect(ids).toEqual(['here']);
    expect(marks().map((span) => span.dataset.quoteIds)).toEqual(['here']);
  });

  it('replaces the marks it made before', () => {
    applyQuoteHighlights(root, [quote('a', 'Ship behind'), quote('b', 'every write.')]);
    applyQuoteHighlights(root, [quote('b', 'every write.')]);

    expect(marks().map((span) => span.dataset.quoteIds)).toEqual(['b', 'b']);

    applyQuoteHighlights(root, []);
    expect(marks()).toEqual([]);
  });
});

describe('clearQuoteHighlights', () => {
  it('leaves the note exactly as it was rendered, text nodes and all', () => {
    const before = root.innerHTML;
    const textNodes = () => {
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      const nodes: string[] = [];
      for (let node = walker.nextNode(); node; node = walker.nextNode())
        nodes.push(node.nodeValue!);
      return nodes;
    };
    const nodesBefore = textNodes();

    applyQuoteHighlights(root, [
      quote('a', 'cache is invalidated on every'),
      quote('b', 'Ship behind a flag. Reads go'),
      quote('c', 'a flag. Reads go to the cache'),
      quote('d', 'Owner\nCanary\tInfra'),
    ]);
    expect(root.innerHTML).not.toBe(before);
    clearQuoteHighlights(root);

    expect(root.innerHTML).toBe(before);
    // Joined up again, not left in the pieces the marks cut.
    expect(textNodes()).toEqual(nodesBefore);
  });

  it('keeps an empty text node beside a mark: it may be an anchor Svelte holds', () => {
    render('<p id="p">Ship behind a flag.</p>');
    const paragraph = root.querySelector('#p')!;
    const anchor = document.createTextNode('');
    paragraph.appendChild(anchor);

    applyQuoteHighlights(root, [quote('a', 'a flag.')]);
    clearQuoteHighlights(root);

    expect(anchor.parentNode).toBe(paragraph);
    expect(anchor.data).toBe('');
    expect(paragraph.textContent).toBe('Ship behind a flag.');
  });

  it('keeps find-in-note matches made inside a mark', () => {
    applyQuoteHighlights(root, [quote('a', 'Ship behind a flag.')]);
    const found = highlightMatches(root, 'behind', 0);
    expect(found.total).toBe(1);

    // Re-marking with the match in place: the match stays, inside the mark.
    applyQuoteHighlights(root, [quote('a', 'Ship behind a flag.')]);
    expect(found.elements[0].isConnected).toBe(true);
    expect(marked('a').join('')).toBe('Ship behind a flag.');

    clearHighlights(root);
    applyQuoteHighlights(root, [quote('a', 'Ship behind a flag.')]);
    expect(marked('a')).toEqual(['Ship behind a flag.']);
  });
});

describe('highlightNoteMatches', () => {
  const quotes = [quote('q', 'cache first.')];

  it('finds a match that runs across the edge of a mark', () => {
    const rendered = root.innerHTML;
    applyQuoteHighlights(root, quotes);
    expect(marked('q')).toEqual(['cache first.']);
    // What a search over the marked note sees: the list item's text in two
    // pieces, cut where the mark begins, and no "the cache" in either.
    expect(root.querySelector('li')!.childNodes).toHaveLength(2);

    const found = highlightNoteMatches(root, root, 'the cache');

    // The sentence under each heading, and the list item's own.
    expect(found.total).toBe(3);
    expect(found.elements.map((match) => match.textContent)).toEqual([
      'The cache',
      'the cache',
      'the cache',
    ]);
    expect(found.elements[1].closest('li')).not.toBeNull();
    // Only the first is the current match, as for any new search.
    expect(found.elements.map((el) => el.classList.contains('search-match-current'))).toEqual([
      true,
      false,
      false,
    ]);

    // The marks go back around and inside the matches, which stay put.
    applyQuoteHighlights(root, quotes);
    expect(found.elements.every((match) => match.isConnected)).toBe(true);
    expect(marked('q')).toEqual(['cache', ' first.']);
    expect(found.elements[1].querySelector('.note-quote-highlight')!.textContent).toBe('cache');
    expect(quoteIdAtTarget(root, found.elements[1].querySelector('.note-quote-highlight'))).toBe(
      'q'
    );

    // The next search starts from whole text again, whatever the last left.
    const next = highlightNoteMatches(root, root, 'cache first');
    expect(next.total).toBe(2);
    expect(found.elements.some((match) => match.isConnected)).toBe(false);

    // And an emptied search leaves the note as it was rendered.
    expect(highlightNoteMatches(root, root, '  ')).toEqual({ total: 0, elements: [] });
    expect(root.innerHTML).toBe(rendered);
    expect(root.querySelector('li')!.childNodes).toHaveLength(1);
  });

  it('searches a container whose note is not rendered', () => {
    expect(highlightNoteMatches(root, undefined, 'canary').total).toBe(1);
  });
});

describe("a quote's marks", () => {
  it('span a range from the first piece to the last', () => {
    applyQuoteHighlights(root, [quote('q1', 'on every write.\n\nReads go')]);

    const range = quoteHighlightRange(root, 'q1')!;
    expect(range.toString().replace(/\s+/g, ' ')).toBe('on every write. Reads go');
    expect(quoteHighlightRange(root, 'missing')).toBeNull();
  });

  it('take a state class together, shared pieces included', () => {
    applyQuoteHighlights(root, [
      quote('long', 'Ship behind a flag. Reads go to'),
      quote('short', 'a flag. Reads go to the cache'),
    ]);
    const lit = () =>
      marks()
        .filter((span) => span.classList.contains('is-active'))
        .map((span) => span.textContent);

    setQuoteHighlightState(root, 'is-active', 'short');
    expect(lit()).toEqual(['a flag. Reads go to', ' the cache']);
    setQuoteHighlightState(root, 'is-active', 'long');
    expect(lit()).toEqual(['Ship behind ', 'a flag. Reads go to']);
    setQuoteHighlightState(root, 'is-active', null);
    expect(lit()).toEqual([]);
  });
});
