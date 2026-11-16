// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import type { QuoteAnchor } from '../sessions/composerDraft';
import {
  ANCHOR_CONTEXT_LENGTH,
  buildNoteTextIndex,
  captureQuoteAnchor,
  CORROBORATED_QUOTE_LENGTH,
  locateQuote,
  MIN_SHARED_CONTEXT,
  positionInIndex,
  quoteAtPassage,
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

  it('does not take a short quote for the same words elsewhere once its passage is rewritten', () => {
    const quote = 'cache';
    const anchor = captureQuoteAnchor(root, rangeOfText('p', quote), quote)!;
    expect(located(locate(quote, anchor))).toBe('cache');

    // The sentence it came from is rewritten. "cache" still stands in three
    // other places, two of them after a "the", as the quoted one was, and
    // before a word starting with the same letter: a few characters of the
    // old surroundings by chance, and not the passage that was replied to.
    root.querySelector('p')!.textContent = 'Entries expire after an hour.';
    const text = buildNoteTextIndex(root).text;
    expect(text.split('cache').length - 1).toBe(3);
    expect(text).toContain('thecachefirst.');

    expect(locate(quote, anchor)).toBeNull();
    // A single occurrence left is no better founded than several.
    render('<p>Entries expire. Warm the cache first.</p>');
    expect(locate(quote, anchor)).toBeNull();
    // Without an anchor there is nothing to hold an occurrence to, as before.
    expect(located(locate(quote))).toBe('cache');
  });

  it('still finds a short quote whose neighbouring text changed on one side only', () => {
    const quote = 'cache';
    const anchor = captureQuoteAnchor(root, rangeOfText('p', quote), quote)!;

    // New words right up against it, before...
    const sentence = root.querySelector('p')!;
    sentence.innerHTML = 'Every regional cache is <strong>invalidated</strong> on every write.';
    let span = locate(quote, anchor)!;
    expect(buildNoteTextIndex(root).text.slice(span.end, span.end + 13)).toBe('isinvalidated');

    // ...or after.
    sentence.textContent = 'The cache was dropped.';
    span = locate(quote, anchor)!;
    const text = buildNoteTextIndex(root).text;
    expect(text.slice(span.start - 15, span.end + 3)).toBe('ArchitectureThecachewas');
  });

  it('takes a long quote wherever it stands word for word, surroundings or not', () => {
    const quote = 'The cache is invalidated on every write.';
    expect(squeezeText(quote).length).toBeGreaterThanOrEqual(CORROBORATED_QUOTE_LENGTH);
    const anchor = captureQuoteAnchor(
      root,
      rangeOf(textIn('p'), 0, textIn('p', 4), ' write.'.length),
      quote
    )!;

    // The sentence moved to a note that shares nothing else with the old one.
    render('<h2>Notes</h2><p>Unrelated. The cache is invalidated on every write. More.</p>');

    expect(located(locate(quote, anchor))).toBe('Thecacheisinvalidatedoneverywrite.');
  });

  it('holds a quote of nearly the whole note to what little surrounds it', () => {
    render('<p>Hi. Short note. Bye</p>');
    const text = textIn('p');
    const quote = 'Short note.';
    const anchor = captureQuoteAnchor(root, rangeOf(text, 4, text, 15), quote)!;
    expect(anchor.prefix.length + anchor.suffix.length).toBeLessThan(MIN_SHARED_CONTEXT);

    expect(located(locate(quote, anchor))).toBe('Shortnote.');
    render('<p>Oh! Short note. Yes</p>');
    expect(locate(quote, anchor)).toBeNull();
  });
});

describe('quoteAtPassage', () => {
  const sentence = 'Reads go to the cache first.';

  function selected(selector: string, needle: string) {
    const anchor = captureQuoteAnchor(root, rangeOfText(selector, needle), needle)!;
    return { quote: needle, headings: [] as string[], anchor };
  }

  it('is the quote already written to exactly the selected passage', () => {
    const inList = { id: 'list', ...selected('ul', sentence) };
    const underRollout = { id: 'rollout', ...selected('p:last-of-type', sentence) };
    const quotes = [inList, underRollout];

    expect(quoteAtPassage(root, quotes, selected('ul', sentence))?.id).toBe('list');
    expect(quoteAtPassage(root, quotes, selected('p:last-of-type', sentence))?.id).toBe('rollout');
  });

  it('is null for a passage inside, around or beside a quote, and for one with no anchor', () => {
    const quotes = [{ id: 'list', ...selected('ul', sentence) }];

    expect(quoteAtPassage(root, quotes, selected('ul', 'the cache'))).toBeNull();
    expect(quoteAtPassage(root, quotes, selected('ul', 'Reads go to the cache'))).toBeNull();
    expect(quoteAtPassage(root, quotes, selected('p:last-of-type', sentence))).toBeNull();
    expect(quoteAtPassage(root, quotes, { quote: sentence })).toBeNull();
    expect(quoteAtPassage(root, [], selected('ul', sentence))).toBeNull();
  });

  it('goes by where the quote is now, not the offset it was stored with', () => {
    const stored = { id: 'rollout', ...selected('p:last-of-type', sentence) };
    // Stored before quotes carried an anchor: found by its text and headings.
    const old = { id: 'old', quote: 'Ship behind a flag.', headings: ['Rollout plan', 'Rollout'] };

    // Text added above moves both passages down the note.
    root.querySelector('p')!.textContent += ' And on every restart, too.';
    const again = selected('p:last-of-type', sentence);
    expect(again.anchor.offset).toBeGreaterThan(stored.anchor.offset);

    expect(quoteAtPassage(root, [old, stored], again)?.id).toBe('rollout');
    expect(
      quoteAtPassage(root, [old, stored], selected('p:last-of-type', 'Ship behind a flag.'))?.id
    ).toBe('old');
  });

  it('is the newest of several quotes on the same words, as a press on their mark is', () => {
    const first = { id: 'first', ...selected('ul', sentence) };
    const second = { id: 'second', ...selected('ul', sentence) };

    expect(quoteAtPassage(root, [first, second], selected('ul', sentence))?.id).toBe('second');
  });
});
