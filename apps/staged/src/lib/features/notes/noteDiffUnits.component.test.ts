import { beforeEach, describe, expect, it, vi } from 'vitest';

import { watchForMissingMedia } from '../../shared/markdown/missingMedia';
import type { PikchrRenderer } from '../../shared/markdown/pikchrRendering';
import { highlightMatches } from '../../shared/textHighlight';
import { computeNoteDiff } from './noteDiff';
import { applyQuoteHighlights } from './noteQuoteHighlights';
import {
  CELL_SEPARATOR,
  extractNoteDiffUnits,
  measureNoteDiffUnits,
  readNoteDiffUnitText,
  type NoteDiffDomUnit,
} from './noteDiffUnits';
import { renderNoteMarkdown } from './noteMarkdown';

const IMAGE = 'staged-media://0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.png';
const OTHER_IMAGE = 'staged-media://0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c.png';
const VIDEO = 'staged-media://0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5d.mp4';

/** Stands in for the WASM renderer: a drawing that differs with its source. */
const pikchrRenderer: PikchrRenderer = (source) => ({
  kind: 'svg',
  svg:
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 40">' +
    `<text x="10" y="20">${source.replace(/[^a-z ]/gi, '')}</text></svg>`,
  width: 100,
  height: 40,
});

function html(markdown: string): string {
  return renderNoteMarkdown(markdown, { pikchrRenderer });
}

/** The note as the viewer shows it: in the page. */
function mount(markdown: string): HTMLElement {
  const root = document.createElement('div');
  root.className = 'markdown-content';
  root.innerHTML = html(markdown);
  document.body.replaceChildren(root);
  return root;
}

/** The note as a previous version is read: parsed, never in the page. */
function parse(markdown: string): HTMLElement {
  const parsed = new DOMParser().parseFromString(`<div>${html(markdown)}</div>`, 'text/html');
  return parsed.body.firstElementChild as HTMLElement;
}

function summary(units: NoteDiffDomUnit[]): [string, string][] {
  return units.map((unit) => [unit.kind, unit.text]);
}

const NOTE = [
  '# Cache design',
  '',
  'The cache is described in the [design doc](https://example.com/design) and in `cache.rs`.',
  'It is warmed at startup.',
  '',
  '- Entries are evicted oldest first',
  '- Evictions are **batched**',
  '  - a hundred at a time',
  '',
  '1. Install the toolchain',
  '',
  '   Use the pinned version.',
  '2. Run the migration',
  '',
  '- [x] Schema reviewed',
  '- [ ] Rollout planned',
  '',
  '| Name | Limit |',
  '| --- | --- |',
  '| uploads | 10 MB |',
  '',
  '```ts',
  'const limit = 10;',
  '  evict(limit);',
  '```',
  '',
  `![The settings page](${IMAGE})`,
  '',
  `![](${VIDEO})`,
  '',
  '---',
  '',
  '> Nothing is written to disk.',
  '',
  '```pikchr',
  'box "Cache"',
  '```',
].join('\n');

beforeEach(() => {
  document.body.replaceChildren();
  vi.spyOn(console, 'info').mockImplementation(() => {});
});

describe('extractNoteDiffUnits', () => {
  it('reads a rendered note as its blocks, list items and table rows', () => {
    expect(summary(extractNoteDiffUnits(mount(NOTE)))).toEqual([
      ['h1', 'Cache design'],
      ['p', 'The cache is described in the design doc and in cache.rs. It is warmed at startup.'],
      ['ul-li', 'Entries are evicted oldest first'],
      ['ul-li', 'Evictions are batched a hundred at a time'],
      ['ol-li', 'Install the toolchain Use the pinned version.'],
      ['ol-li', 'Run the migration'],
      ['ul-li', 'Schema reviewed'],
      ['ul-li', 'Rollout planned'],
      ['tr', 'Name\tLimit'],
      ['tr', 'uploads\t10 MB'],
      ['code', 'const limit = 10;\n  evict(limit);\n'],
      ['media', 'The settings page'],
      ['media', ''],
      ['hr', ''],
      ['blockquote', 'Nothing is written to disk.'],
      ['diagram', ''],
    ]);
  });

  it('names each unit by the element on screen', () => {
    const root = mount(NOTE);
    const units = extractNoteDiffUnits(root);

    expect(units.map((unit) => unit.element.tagName)).toEqual([
      'H1',
      'P',
      'LI',
      'LI',
      'LI',
      'LI',
      'LI',
      'LI',
      'TR',
      'TR',
      'PRE',
      'FIGURE',
      'FIGURE',
      'HR',
      'BLOCKQUOTE',
      'FIGURE',
    ]);
    expect(units.every((unit) => root.contains(unit.element))).toBe(true);
    // A nested list belongs to the item it is in.
    expect(units[3].element.querySelector('ul')).not.toBeNull();
  });

  it('skips the empty paragraphs the parser leaves around a media figure', () => {
    const root = mount(`Before the picture.\n\n![Shot](${IMAGE})\n\nAfter the picture.`);

    // The figure's paragraph was closed ahead of it and again after it.
    expect(root.querySelectorAll('p')).toHaveLength(4);
    expect(summary(extractNoteDiffUnits(root))).toEqual([
      ['p', 'Before the picture.'],
      ['media', 'Shot'],
      ['p', 'After the picture.'],
    ]);
  });

  it('reads a figure the same once it stands for media that is gone', () => {
    const root = mount(`Before\n\n![Shot](${IMAGE})\n\n![Clip](${VIDEO})\n\nAfter`);
    const read = () =>
      extractNoteDiffUnits(root).map(({ kind, text, signature }) => ({ kind, text, signature }));
    const before = read();
    watchForMissingMedia(root);

    for (const media of root.querySelectorAll('.markdown-media > :is(img, video)')) {
      media.dispatchEvent(new Event('error'));
    }

    expect(root.querySelectorAll('[data-media-missing]')).toHaveLength(2);
    expect(read()).toEqual(before);
  });

  it('reads what a paragraph said after its figure as part of the figure', () => {
    const root = mount(`Before ![Shot](${IMAGE}) after *the* fact\n\nNext.`);
    const units = extractNoteDiffUnits(root);

    expect(summary(units)).toEqual([
      ['p', 'Before'],
      ['media', 'Shot after the fact'],
      ['p', 'Next.'],
    ]);
    expect(units[1].element.tagName).toBe('FIGURE');
  });

  it('reads the two versions alike, in the page or parsed', () => {
    const live = extractNoteDiffUnits(mount(NOTE));
    const parsed = extractNoteDiffUnits(parse(NOTE));

    const plain = (units: NoteDiffDomUnit[]) =>
      units.map(({ kind, text, signature }) => ({ kind, text, signature }));
    expect(plain(parsed)).toEqual(plain(live));
    expect(computeNoteDiff(parsed, live).hunks).toEqual([]);
  });

  it('is not changed by marks put around the note’s words', () => {
    const markdown = `${NOTE}\n\nBefore ![Shot](${IMAGE}) the cache again`;
    const root = mount(markdown);
    const before = summary(extractNoteDiffUnits(root));

    // Find-in-note wraps each match in a <mark>, splitting its text node:
    // inside blocks, in the text left loose after a figure, and in a
    // diagram's label.
    expect(highlightMatches(root, 'cache', 0).total).toBeGreaterThan(2);
    expect(root.querySelector(':scope > mark')).not.toBeNull();
    expect(root.querySelector('svg mark')).not.toBeNull();

    const marked = extractNoteDiffUnits(root);
    expect(summary(marked)).toEqual(before);
    expect(computeNoteDiff(extractNoteDiffUnits(parse(markdown)), marked).hunks).toEqual([]);
  });

  describe('a table row', () => {
    const table = (...rows: string[]) => ['| One | Two | Three |', '| --- | --- | --- |', ...rows];
    /** The units of a three-column table with these rows under its header. */
    const rowsOf = (...rows: string[]) =>
      extractNoteDiffUnits(parse(table(...rows).join('\n'))).slice(1);
    /** The kinds of change between two tables that differ in their rows alone. */
    const changes = (before: string[], after: string[]) =>
      computeNoteDiff(
        extractNoteDiffUnits(parse(table(...before).join('\n'))),
        extractNoteDiffUnits(mount(table(...after).join('\n')))
      ).hunks.map((hunk) => [hunk.kind, hunk.previousIndices, hunk.currentIndices]);

    it('keeps its cells apart in its text', () => {
      const [row] = rowsOf('| a b | c | d |');

      expect(row.text).toBe(['a b', 'c', 'd'].join(CELL_SEPARATOR));
    });

    it('reads as changed when words moved from one cell to the next', () => {
      // The words of the row are the same, in the same order, before and after.
      const [before] = rowsOf('| a b | c | d |');
      const [after] = rowsOf('| a | b c | d |');
      expect(after.text.split(/\s/)).toEqual(before.text.split(/\s/));

      expect(after.text).not.toBe(before.text);
      expect(
        changes(['| a b | c | d |', '| x | y | z |'], ['| a | b c | d |', '| x | y | z |'])
      ).toEqual([['modified', [1], [1]]]);
    });

    it('reads as changed when a whole cell moved to the next column', () => {
      const [before] = rowsOf('| a |  | b |');
      const [after] = rowsOf('|  | a | b |');

      expect(before.text).toBe(`a${CELL_SEPARATOR}${CELL_SEPARATOR}b`);
      expect(after.text).toBe(`${CELL_SEPARATOR}a${CELL_SEPARATOR}b`);
      expect(changes(['| a |  | b |'], ['|  | a | b |'])).toEqual([['modified', [1], [1]]]);
    });

    it('tells a cell moved from the first column to the last from the row it left', () => {
      // Both read "a b" once the ends are trimmed, which a line diff does.
      expect(changes(['|  | a | b |'], ['| a | b |  |'])).toEqual([['modified', [1], [1]]]);
    });

    it('reads the same whatever whitespace its markup has around and inside its cells', () => {
      const row = (markup: string) => {
        const root = document.createElement('div');
        root.innerHTML = `<table><tbody>${markup}</tbody></table>`;
        return extractNoteDiffUnits(root)[0].text;
      };

      expect(row('<tr><td>a b</td><td>c</td></tr>')).toBe(`a b${CELL_SEPARATOR}c`);
      expect(row('<tr>\n  <td> a\n b </td>\n  <td>\n c </td>\n</tr>')).toBe(
        `a b${CELL_SEPARATOR}c`
      );
    });

    it('is unchanged by find-in-note’s marks and a reply’s quote spans inside its cells', () => {
      const markdown = [
        ...table('| uploads are capped | 10 MB each | per file |', '| x | y | z |'),
        '',
        'After the table.',
      ].join('\n');
      const previous = extractNoteDiffUnits(parse(markdown));
      const root = mount(markdown);
      const before = summary(extractNoteDiffUnits(root));

      // A quote that runs from one cell into the next, and a match in each of them.
      expect(
        applyQuoteHighlights(root, [
          { id: 'q1', quote: 'capped\t10 MB', headings: [] },
          { id: 'q2', quote: 'per', headings: [] },
        ])
      ).toEqual(['q1', 'q2']);
      expect(highlightMatches(root, 'a', 0).total).toBeGreaterThan(2);
      expect(root.querySelectorAll('td .note-quote-highlight').length).toBeGreaterThan(1);
      expect(root.querySelectorAll('td mark').length).toBeGreaterThan(2);

      const marked = extractNoteDiffUnits(root);
      expect(summary(marked)).toEqual(before);
      expect(marked.map((unit) => unit.signature)).toEqual(previous.map((unit) => unit.signature));
      expect(computeNoteDiff(previous, marked).hunks).toEqual([]);
    });

    it('still pairs a row with one word changed, and finds the word', () => {
      const previous = extractNoteDiffUnits(
        parse(table('| uploads | 10 MB | per file |').join('\n'))
      );
      const current = extractNoteDiffUnits(
        mount(table('| uploads | 25 MB | per file |').join('\n'))
      );
      const { hunks } = computeNoteDiff(previous, current);

      expect(hunks).toHaveLength(1);
      expect(hunks[0].ops).toMatchObject([{ type: 'modified', wholeUnit: false }]);
      const [op] = hunks[0].ops;
      if (op.type !== 'modified') throw new Error('expected a modified pair');
      const words = (text: string, ranges: { start: number; end: number }[]) =>
        ranges.map(({ start, end }) => text.slice(start, end));
      expect(words(previous[1].text, op.previousRanges)).toEqual(['10']);
      expect(words(current[1].text, op.currentRanges)).toEqual(['25']);
    });
  });

  describe('signature', () => {
    /** The signature of the one block `markdown` renders. */
    function signatureOf(markdown: string): string {
      const units = extractNoteDiffUnits(parse(markdown));
      expect(units).toHaveLength(1);
      return units[0].signature;
    }

    it('is empty for a block of text alone', () => {
      expect(signatureOf('Plain *text* only.')).toBe('');
    });

    it.each([
      [
        'a link’s target',
        'See [the doc](https://example.com/a).',
        'See [the doc](https://example.com/b).',
      ],
      ['the image', `![Shot](${IMAGE})`, `![Shot](${OTHER_IMAGE})`],
      ['a code block’s language', '```ts\nlet a;\n```', '```js\nlet a;\n```'],
      ['a ticked task', '- [ ] Ship it', '- [x] Ship it'],
      ['a diagram’s drawing', '```pikchr\nbox "A"\n```', '```pikchr\nbox "B"\n```'],
    ])('tells %s apart where the text is the same', (_what, before, after) => {
      const [previous] = extractNoteDiffUnits(parse(before));
      const [current] = extractNoteDiffUnits(parse(after));

      expect(current.text).toBe(previous.text);
      expect(current.kind).toBe(previous.kind);
      expect(current.signature).not.toBe(previous.signature);
      expect(signatureOf(before)).toBe(previous.signature);
    });

    it('does not depend on the address of the document the note is in', () => {
      const markdown = 'See [the doc](/relative/path) and ![remote](https://example.com/a.png).';
      const [live] = extractNoteDiffUnits(mount(markdown));

      expect(live.signature).toBe(signatureOf(markdown));
    });
  });

  it('finds the changes between a parsed previous version and the note on screen', () => {
    const previous = extractNoteDiffUnits(parse(NOTE));
    const root = mount(
      NOTE.replace('oldest first', 'newest first')
        .replace('| uploads | 10 MB |', '| uploads | 25 MB |')
        .replace('box "Cache"', 'box "Store"')
        .replace('---\n\n', '')
        .replace('```\n\n![The', '```\n\n## Screens\n\nAs of this week.\n\n![The')
    );
    const current = extractNoteDiffUnits(root);
    const diff = computeNoteDiff(previous, current);

    expect(
      diff.hunks.map((hunk) => [
        hunk.kind,
        hunk.previousIndices.map((index) => previous[index].kind),
        hunk.currentIndices.map((index) => current[index].kind),
      ])
    ).toEqual([
      ['modified', ['ul-li'], ['ul-li']],
      ['modified', ['tr'], ['tr']],
      ['added', [], ['h2', 'p']],
      ['removed', ['hr'], []],
      ['modified', ['diagram'], ['diagram']],
    ]);
    // The removed rule stood where the quote now is.
    expect(current[diff.hunks[3].currentStart].kind).toBe('blockquote');
    // The redrawn diagram is one block changed, though it has no text to compare.
    expect(diff.hunks[4].ops).toMatchObject([{ type: 'modified', wholeUnit: true }]);
  });
});

describe('readNoteDiffUnitText', () => {
  function block(markup: string): HTMLElement {
    const root = document.createElement('div');
    root.innerHTML = markup;
    document.body.replaceChildren(root);
    return root.firstElementChild as HTMLElement;
  }

  it('collapses whitespace, trims the ends and reads a line break as a space', () => {
    const element = block('<p>\n  The   cache<br>is\twarm. \n</p>');

    expect(readNoteDiffUnitText(element).text).toBe('The cache is warm.');
  });

  it('keeps the whitespace of code', () => {
    const element = block('<pre><code>if (a) {\n    b();\n}\n</code></pre>');

    expect(readNoteDiffUnitText(element).text).toBe('if (a) {\n    b();\n}\n');
  });

  it('leaves out a diagram’s labels and anything in a script or style', () => {
    const element = block(
      '<div>Shown<svg><text>label</text></svg><style>p { color: red }</style><script>let a;</script></div>'
    );

    expect(readNoteDiffUnitText(element).text).toBe('Shown');
  });

  it('parts the cells of a table row, with no space kept beside the parting', () => {
    const row = block(
      '<table><tbody><tr>\n<td> The <em>cache</em> </td>\n<td></td>\n<td>is warm </td>\n</tr></tbody></table>'
    ).querySelector('tr') as HTMLElement;
    const read = readNoteDiffUnitText(row, true);

    expect(read.text).toBe(`The cache${CELL_SEPARATOR}${CELL_SEPARATOR}is warm`);
    // The parting is in no text node, and every other character is where it says.
    for (let index = 0; index < read.text.length; index++) {
      if (read.text[index] === CELL_SEPARATOR) {
        expect(read.nodeOf[index]).toBe(-1);
      } else {
        expect(read.nodes[read.nodeOf[index]].data[read.offsetOf[index]]).toBe(read.text[index]);
      }
    }
  });

  it('reports no positions unless asked', () => {
    const read = readNoteDiffUnitText(block('<p>The cache</p>'));

    expect(read.nodeOf).toEqual([]);
    expect(read.offsetOf).toEqual([]);
  });

  it('places every character in the text node that holds it', () => {
    const element = block('<p>  The <strong>warm</strong>   cache<br>stays</p>');
    const read = readNoteDiffUnitText(element, true);

    expect(read.text).toBe('The warm cache stays');
    expect(read.nodeOf).toHaveLength(read.text.length);
    const at = (index: number) =>
      read.nodeOf[index] === -1 ? null : read.nodes[read.nodeOf[index]].data[read.offsetOf[index]];
    // Each character is where it says it is; a collapsed run is its first space.
    for (let index = 0; index < read.text.length; index++) {
      if (read.nodeOf[index] === -1) continue;
      expect(at(index)).toBe(read.text[index]);
    }
    expect(read.nodes[read.nodeOf[4]].parentElement?.tagName).toBe('STRONG');
    expect(read.offsetOf[8]).toBe(0);
    // The space a <br> stands for is in no text node.
    expect(read.text[14]).toBe(' ');
    expect(read.nodeOf[14]).toBe(-1);
  });
});

describe('measureNoteDiffUnits', () => {
  /** jsdom lays nothing out: give an element the offsets a browser would report. */
  function layout(
    element: HTMLElement,
    box: { top: number; height?: number; parent: HTMLElement | null; border?: number }
  ) {
    Object.defineProperties(element, {
      offsetTop: { configurable: true, value: box.top },
      offsetHeight: { configurable: true, value: box.height ?? 0 },
      offsetParent: { configurable: true, value: box.parent },
      clientTop: { configurable: true, value: box.border ?? 0 },
    });
  }

  function scene() {
    const dialog = document.createElement('div');
    const scroller = document.createElement('div');
    const root = mount('First paragraph.\n\n| a |\n| - |\n| 1 |');
    scroller.append(root);
    dialog.append(scroller);
    document.body.replaceChildren(dialog);
    const units = extractNoteDiffUnits(root);
    const table = root.querySelector('table') as HTMLElement;
    return { dialog, scroller, units, table };
  }

  it('measures each unit from the top of the scrolled content', () => {
    const { dialog, scroller, units, table } = scene();
    const [paragraph, head, row] = units.map((unit) => unit.element);
    // The scroller is the units' offset parent, 80px down a dialog.
    layout(dialog, { top: 200, parent: null });
    layout(scroller, { top: 80, parent: dialog });
    layout(paragraph, { top: 16, height: 24, parent: scroller });
    // Rows are offset from their table, inside its border.
    layout(table, { top: 56, height: 61, parent: scroller, border: 1 });
    layout(head, { top: 0, height: 30, parent: table });
    layout(row, { top: 30, height: 29, parent: table });

    expect(measureNoteDiffUnits(units, scroller)).toEqual([
      { top: 16, height: 24 },
      { top: 57, height: 30 },
      { top: 87, height: 29 },
    ]);
  });

  it('measures the same when the container is not the units’ offset parent', () => {
    const { dialog, scroller, units, table } = scene();
    const [paragraph, head, row] = units.map((unit) => unit.element);
    // Nothing positioned between the units and the dialog: offsets are the dialog's.
    layout(dialog, { top: 200, parent: null });
    layout(scroller, { top: 80, parent: dialog });
    layout(paragraph, { top: 96, height: 24, parent: dialog });
    layout(table, { top: 136, height: 61, parent: dialog, border: 1 });
    layout(head, { top: 0, height: 30, parent: table });
    layout(row, { top: 30, height: 29, parent: table });

    expect(measureNoteDiffUnits(units, scroller)).toEqual([
      { top: 16, height: 24 },
      { top: 57, height: 30 },
      { top: 87, height: 29 },
    ]);
  });

  it('measures from inside the container’s border', () => {
    const { dialog, scroller, units } = scene();
    const [paragraph] = units.map((unit) => unit.element);
    layout(dialog, { top: 0, parent: null });
    layout(scroller, { top: 80, parent: dialog, border: 2 });
    layout(paragraph, { top: 98, height: 24, parent: dialog });

    expect(measureNoteDiffUnits(units.slice(0, 1), scroller)).toEqual([{ top: 16, height: 24 }]);
  });
});
