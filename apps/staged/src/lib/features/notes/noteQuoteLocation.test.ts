// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import type { QuoteAnchor } from '../sessions/composerDraft';
import {
  ANCHOR_CONTEXT_LENGTH,
  buildNoteTextIndex,
  captureQuoteAnchor,
  locateQuote,
  positionInIndex,
  squeezeText,
  type NoteTextSpan,
} from './noteQuoteLocation';

// A quote is found again in the rendered note by its text with whitespace
// left out, and told from the same words elsewhere by where it was selected.

let root: HTMLElement;

function render(html: string) {
  root = document.createElement('div');
  root.innerHTML = html;
  document.body.replaceChildren(root);
}

function textIn(selector: string, nth = 0): Text {
  const walker = document.createTreeWalker(root.querySelector(selector)!, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode();
  for (let i = 0; i < nth && node; i++) node = walker.nextNode();
  if (!node) throw new Error(`no text ${nth} in ${selector}`);
  return node as Text;
}

function rangeOf(start: Node, startOffset: number, end: Node, endOffset: number): Range {
  const range = document.createRange();
  range.setStart(start, startOffset);
  range.setEnd(end, endOffset);
  return range;
}

/** The range of `needle`'s `nth` occurrence within one text node of `selector`. */
function rangeOfText(selector: string, needle: string, nth = 0): Range {
  const walker = document.createTreeWalker(root.querySelector(selector)!, NodeFilter.SHOW_TEXT);
  let seen = 0;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const data = (node as Text).data;
    for (let at = data.indexOf(needle); at !== -1; at = data.indexOf(needle, at + 1)) {
      if (seen++ === nth) return rangeOf(node, at, node, at + needle.length);
    }
  }
  throw new Error(`no occurrence ${nth} of ${needle}`);
}

/** The note text a located span covers, squeezed. */
function located(span: NoteTextSpan | null): string | null {
  return span ? buildNoteTextIndex(root).text.slice(span.start, span.end) : null;
}

function locate(quote: string, anchor?: QuoteAnchor | null, headings: string[] = []) {
  return locateQuote(buildNoteTextIndex(root), {
    quote,
    headings,
    ...(anchor ? { anchor } : {}),
  });
}

const NOTE = `
  <h1>Rollout plan</h1>
  <h2>Architecture</h2>
  <p>The cache is <strong>invalidated</strong> on <em>every</em> write.</p>
  <ul>
    <li>Reads go to the cache first.</li>
    <li>Writes go to the <a href="https://example.com">primary store</a>.</li>
  </ul>
  <h2>Rollout</h2>
  <p>Ship behind a flag. Reads go to the cache first.</p>
  <pre><code>fn main() {
    cache.invalidate();
}</code></pre>
  <table>
    <tr><th>Stage</th><th>Owner</th></tr>
    <tr><td>Canary</td><td>Infra</td></tr>
  </table>
`;

beforeEach(() => render(NOTE));

describe('buildNoteTextIndex', () => {
  it('holds the note text without whitespace, and where each character is', () => {
    const index = buildNoteTextIndex(root);

    expect(index.text.startsWith('RolloutplanArchitectureThecacheisinvalidatedon')).toBe(true);
    // Markup indentation holds no characters, so it is in no node of the index.
    expect(index.nodes.every((node) => squeezeText(node.data).length > 0)).toBe(true);

    const at = index.text.indexOf('invalidated');
    const position = positionInIndex(index, at + 2);
    expect(index.nodes[position.node]).toBe(textIn('strong'));
    expect(position.offset).toBe(2);
    // Whitespace inside a node is skipped over: the "i" of "is".
    const is = positionInIndex(index, index.text.indexOf('cacheis') + 5);
    expect(index.nodes[is.node].data.slice(is.offset, is.offset + 2)).toBe('is');
  });
});

describe('captureQuoteAnchor', () => {
  it('keeps the text on either side of the selection and how far in it began', () => {
    const range = rangeOfText('ul', 'Reads go to the cache first.');
    const anchor = captureQuoteAnchor(root, range, 'Reads go to the cache first.')!;
    const text = buildNoteTextIndex(root).text;

    expect(anchor.offset).toBe(text.indexOf('Readsgotothecachefirst.'));
    expect(anchor.prefix).toBe(text.slice(anchor.offset - ANCHOR_CONTEXT_LENGTH, anchor.offset));
    expect(anchor.prefix.endsWith('oneverywrite.')).toBe(true);
    expect(anchor.suffix.startsWith('Writesgotothe')).toBe(true);
    expect(anchor.suffix).toHaveLength(ANCHOR_CONTEXT_LENGTH);
    // The selection read the same text the nodes hold: nothing more to keep.
    expect(anchor.text).toBeUndefined();
  });

  it('reads boundaries between nodes and in markup indentation', () => {
    const paragraph = root.querySelector('p')!;
    // From before the paragraph's first child to the whitespace after it.
    const range = rangeOf(paragraph, 0, paragraph.nextSibling!, 1);
    const anchor = captureQuoteAnchor(root, range, 'The cache is invalidated on every write.')!;

    expect(anchor.prefix).toBe('RolloutplanArchitecture');
    expect(anchor.suffix.startsWith('Readsgoto')).toBe(true);
    expect(anchor.text).toBeUndefined();
  });

  it('keeps the passage as the nodes spell it when the selection read it differently', () => {
    // A browser that writes the list's bullets into the selection's text.
    const range = rangeOf(textIn('li'), 0, textIn('li:last-child a'), 7);
    const anchor = captureQuoteAnchor(
      root,
      range,
      '• Reads go to the cache first.\n• Writes go to the primary'
    )!;

    expect(anchor.text).toBe('Readsgotothecachefirst.Writesgototheprimary');
  });

  it('is null for a range with no text of the note in it, or outside it', () => {
    const paragraph = root.querySelector('p')!;
    const gap = paragraph.nextSibling!;
    expect(captureQuoteAnchor(root, rangeOf(gap, 0, gap, 1), '')).toBeNull();

    const outside = document.createElement('p');
    outside.textContent = 'The cache is elsewhere.';
    document.body.appendChild(outside);
    expect(
      captureQuoteAnchor(root, rangeOf(outside.firstChild!, 0, outside.firstChild!, 9), 'The cache')
    ).toBeNull();
  });
});

describe('locateQuote', () => {
  it('finds a passage running through inline elements', () => {
    expect(located(locate('cache is invalidated on every write.'))).toBe(
      'cacheisinvalidatedoneverywrite.'
    );
  });

  it('finds a passage running across blocks, list items and a link', () => {
    // As a selection reads it: a line break at each block edge.
    const quote = 'on every write.\n\nReads go to the cache first.\nWrites go to the primary';

    expect(located(locate(quote))).toBe('oneverywrite.Readsgotothecachefirst.Writesgototheprimary');
  });

  it('finds passages in a code block and across table cells', () => {
    expect(located(locate('fn main() {\n    cache.invalidate();\n}'))).toBe(
      'fnmain(){cache.invalidate();}'
    );
    // Cells are tab-separated in a selection's text.
    expect(located(locate('Stage\tOwner\nCanary\tInfra'))).toBe('StageOwnerCanaryInfra');
  });

  it('takes the occurrence the user selected, not the first', () => {
    const text = buildNoteTextIndex(root).text;
    const first = text.indexOf('Readsgotothecachefirst.');
    const second = text.indexOf('Readsgotothecachefirst.', first + 1);
    const quote = 'Reads go to the cache first.';

    const inList = captureQuoteAnchor(root, rangeOfText('ul', quote), quote);
    const underRollout = captureQuoteAnchor(root, rangeOfText('p:last-of-type', quote), quote);

    expect(locate(quote, inList)?.start).toBe(first);
    expect(locate(quote, underRollout)?.start).toBe(second);
    // With nothing to go by, the first.
    expect(locate(quote)?.start).toBe(first);
  });

  it('tells apart occurrences whose surroundings are the same, by where they began', () => {
    render(`<p>${'ha '.repeat(40)}</p>`);
    const text = textIn('p');
    // The twentieth "ha": identical text for 32 characters either side.
    const range = rangeOf(text, 19 * 3, text, 19 * 3 + 2);
    const anchor = captureQuoteAnchor(root, range, 'ha')!;

    expect(locate('ha', anchor)).toEqual({ start: 38, end: 40 });
  });

  it('follows the passage when text is added or removed elsewhere in the note', () => {
    const quote = 'Reads go to the cache first.';
    const anchor = captureQuoteAnchor(root, rangeOfText('p:last-of-type', quote), quote)!;

    // An agent rewrites the top of the note: every offset after it moves.
    root.querySelector('p')!.textContent = 'Reads go to the cache first. Entirely new opening.';
    const span = locate(quote, anchor)!;
    const text = buildNoteTextIndex(root).text;

    expect(span.start).not.toBe(anchor.offset);
    expect(text.slice(span.start - 16, span.start)).toBe('Shipbehindaflag.');
  });

  it('goes by the headings of a quote stored without an anchor', () => {
    const text = buildNoteTextIndex(root).text;
    const first = text.indexOf('Readsgotothecachefirst.');
    const second = text.indexOf('Readsgotothecachefirst.', first + 1);
    const quote = 'Reads go to the cache first.';

    expect(locate(quote, null, ['Rollout plan', 'Rollout'])?.start).toBe(second);
    expect(locate(quote, null, ['Rollout plan', 'Architecture'])?.start).toBe(first);
    // Headings that match nothing (the note was retitled): the first occurrence.
    expect(locate(quote, null, ['Somewhere else'])?.start).toBe(first);
  });

  it('leaves the ignored title heading out of the paths it compares', () => {
    const index = buildNoteTextIndex(root);
    const second = index.text.lastIndexOf('Readsgotothecachefirst.');

    const span = locateQuote(
      index,
      { quote: 'Reads go to the cache first.', headings: ['Rollout'] },
      { ignore: (heading) => heading.tagName === 'H1' }
    );

    expect(span?.start).toBe(second);
  });

  it('uses the passage as the nodes spelled it, and the quote when that is gone', () => {
    const range = rangeOf(textIn('li'), 0, textIn('li:last-child a'), 7);
    const quote = '• Reads go to the cache first.\n• Writes go to the primary';
    const anchor = captureQuoteAnchor(root, range, quote)!;

    expect(located(locate(quote, anchor))).toBe('Readsgotothecachefirst.Writesgototheprimary');

    // The note now spells out the bullets, and no longer holds the old run of text.
    render('<p>• Reads go to the cache first. • Writes go to the primary store.</p>');
    expect(located(locate(quote, anchor))).toBe('•Readsgotothecachefirst.•Writesgototheprimary');
  });

  it('is null when the note no longer holds the text, or the quote is blank', () => {
    const quote = 'Reads go to the cache first.';
    const anchor = captureQuoteAnchor(root, rangeOfText('ul', quote), quote);

    for (const item of root.querySelectorAll('li, p')) item.textContent = 'Rewritten.';
    expect(locate(quote, anchor)).toBeNull();
    expect(locate(quote)).toBeNull();
    expect(locate('  \n ')).toBeNull();

    render('');
    expect(locate(quote, anchor)).toBeNull();
  });
});
