/**
 * Reading a rendered note as the blocks a note diff compares
 * (see noteDiff.ts), and measuring where they are.
 *
 * The units are the children of the note's rendered markdown. A list is
 * taken an item at a time and a table a row at a time, so one changed item
 * does not mark the whole list; anything nested deeper belongs to its
 * top-level block. The same reading is run over both versions: the current
 * one in the live note, where a unit's element is the element on screen,
 * and the previous one in whatever document its HTML was parsed into.
 *
 * A unit's text is what its text nodes hold, with whitespace collapsed as
 * the browser collapses it and a `<br>` read as a space. Markup that leaves
 * the words alone therefore does not change it: the spans around a quoted
 * passage, find-in-note's marks, a list re-indented in the HTML. Code keeps
 * its whitespace, where it is content. A table row's cells are kept apart
 * (`CELL_SEPARATOR`), since which cell a word is in is content too.
 * `readNoteDiffUnitText` can also report where each character of that text
 * is in the DOM, which is how a range of it is turned back into a `Range`
 * (noteDiffHighlights.ts).
 */

import type { NoteDiffUnit } from './noteDiff';
import type { NoteDiffUnitBox } from './noteDiffScroll';

/** A unit, with the element it was read from. */
export interface NoteDiffDomUnit extends NoteDiffUnit {
  /** The block itself: what is marked and measured for the unit. */
  element: HTMLElement;
}

/**
 * Elements that sit in a line of text. One found among the note's blocks
 * is not a block: see `trailingNodes`.
 */
const INLINE_TAGS = new Set([
  'A',
  'ABBR',
  'B',
  'BR',
  'CITE',
  'CODE',
  'DEL',
  'EM',
  'I',
  'IMG',
  'INPUT',
  'INS',
  'KBD',
  'MARK',
  'Q',
  'S',
  'SAMP',
  'SMALL',
  'SPAN',
  'STRONG',
  'SUB',
  'SUP',
  'U',
  'VAR',
]);

/** Subtrees whose text is not the note's words: a diagram's labels are part of its drawing. */
const TEXTLESS_TAGS = new Set(['SVG', 'SCRIPT', 'STYLE']);

/** Content with no text of its own, which still makes a block worth comparing. */
const EMBEDDED = 'img, video, audio, svg, hr, input';

function isElement(node: Node): node is HTMLElement {
  return node.nodeType === 1;
}

function isText(node: Node): node is Text {
  return node.nodeType === 3;
}

function isInline(node: Node): boolean {
  return isText(node) || (isElement(node) && INLINE_TAGS.has(node.tagName.toUpperCase()));
}

/**
 * The text and inline elements that follow a block as its siblings, up to
 * the next block.
 *
 * A media figure is emitted inside its paragraph, and the HTML parser ends
 * the paragraph where the figure starts. Whatever the paragraph went on to
 * say is left after the figure with no element of its own, so it is read as
 * part of the figure's unit: the block it is laid out with. Between other
 * blocks this is the whitespace the HTML is indented with, which adds
 * nothing.
 */
function trailingNodes(element: Element): Node[] {
  const nodes: Node[] = [];
  for (let node = element.nextSibling; node; node = node.nextSibling) {
    if (isElement(node) && !isInline(node)) break;
    if (isInline(node)) nodes.push(node);
  }
  return nodes;
}

/**
 * What stands between two cells of a table row in the row's text, in place
 * of the space the markup between them would have left.
 *
 * Without it a row is the same text wherever its cells are divided: `a b`
 * beside `c` reads as `a` beside `b c` does, and words a revision moved to
 * another column were no change at all. A tab, as a row copied out of a
 * table has between its cells and as a reply's quote of one has. Rendered
 * text holds no tab of its own outside code, where a tab collapses to a
 * space, and the diff treats it as it treats a space: a word's highlight
 * does not start or end on it, and a row that differs in nothing else is
 * marked whole (`wordRanges` in noteDiff.ts).
 */
export const CELL_SEPARATOR = '\t';

/** The cells of a table row. */
const CELL_TAGS = new Set(['TD', 'TH']);

/** The whitespace HTML collapses: space, tab, line feed, form feed, carriage return. */
function isCollapsible(code: number): boolean {
  return code === 0x20 || code === 0x09 || code === 0x0a || code === 0x0c || code === 0x0d;
}

/** A unit's text, and where each character of it came from. */
export interface NoteDiffUnitText {
  text: string;
  /** The text nodes that hold the text, in order. */
  nodes: Text[];
  /**
   * For each character of `text`, the index in `nodes` of the node holding
   * it, or -1 for one no text node holds (the space of a `<br>`, the parting
   * between two cells). Empty unless positions were asked for.
   */
  nodeOf: number[];
  /** For each character of `text`, its offset in that node. */
  offsetOf: number[];
}

/**
 * Read the text of the unit at `element`: the element's own and that of the
 * inline content trailing it.
 *
 * A run of whitespace is one space, and none is kept at either end; a
 * collapsed run is placed at its first character. Text inside a `<pre>` is
 * kept as it is. Each cell of a table row after its first is led by a
 * `CELL_SEPARATOR`, an empty cell too, with no space kept either side of
 * it: the row reads the same however its markup is spaced.
 */
export function readNoteDiffUnitText(element: Element, withPositions = false): NoteDiffUnitText {
  const nodes: Text[] = [];
  const nodeOf: number[] = [];
  const offsetOf: number[] = [];
  let text = '';
  // A space owed to the text so far, written once more text follows it.
  let pending: { node: number; offset: number } | null = null;
  // Nothing has been written since a cell began: a cell's text starts at its first word.
  let cellStart = false;

  const write = (char: string, node: number, offset: number) => {
    text += char;
    cellStart = false;
    if (withPositions) {
      nodeOf.push(node);
      offsetOf.push(offset);
    }
  };
  const owe = (node: number, offset: number) => {
    if (text.length === 0 || cellStart) return;
    // A place in a text node is kept over a `<br>`'s, which has none.
    if (!pending || (pending.node === -1 && node !== -1)) pending = { node, offset };
  };

  const visit = (node: Node, preformatted: boolean) => {
    if (isText(node)) {
      const at = nodes.length;
      nodes.push(node);
      const data = node.data;
      for (let offset = 0; offset < data.length; offset++) {
        if (!preformatted && isCollapsible(data.charCodeAt(offset))) {
          owe(at, offset);
          continue;
        }
        if (pending) {
          write(' ', pending.node, pending.offset);
          pending = null;
        }
        write(data[offset], at, offset);
      }
      return;
    }
    if (!isElement(node)) return;
    const tag = node.tagName.toUpperCase();
    if (TEXTLESS_TAGS.has(tag)) return;
    if (tag === 'BR') {
      owe(-1, 0);
      return;
    }
    if (CELL_TAGS.has(tag) && node.previousElementSibling) {
      // The space owed to the cell before goes: its text ended at its last word.
      pending = null;
      write(CELL_SEPARATOR, -1, 0);
      cellStart = true;
    }
    const inPre = preformatted || tag === 'PRE';
    for (let child = node.firstChild; child; child = child.nextSibling) visit(child, inPre);
  };

  visit(element, false);
  for (const node of trailingNodes(element)) visit(node, false);
  return { text, nodes, nodeOf, offsetOf };
}

/** 32-bit FNV-1a, as eight hex digits. */
function hash(value: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < value.length; i++) {
    h ^= value.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, '0');
}

const SVG_NAMESPACE = 'http://www.w3.org/2000/svg';

/**
 * A diagram's drawing as a string: its elements, their attributes and its
 * labels.
 *
 * Not `outerHTML`. Find-in-note marks a match inside a diagram's label as
 * it does anywhere else, with an HTML `<mark>` that splits the label's text
 * node, and the diagram would read as redrawn for as long as the search was
 * open. An HTML element in a drawing is such a mark, so only what it holds
 * is kept.
 */
function drawingOf(node: Node): string {
  if (isText(node)) return node.data;
  if (!isElement(node)) return '';
  let inner = '';
  for (let child = node.firstChild; child; child = child.nextSibling) inner += drawingOf(child);
  if (node.namespaceURI !== SVG_NAMESPACE) return inner;
  const attributes = [...node.attributes].map(({ name, value }) => `${name}="${value}"`);
  return `<${[node.tagName, ...attributes].join(' ')}>${inner}</${node.tagName}>`;
}

function matchesWithin(scopes: Element[], selector: string): Element[] {
  return scopes.flatMap((scope) => [
    ...(scope.matches(selector) ? [scope] : []),
    ...scope.querySelectorAll(selector),
  ]);
}

/**
 * What a unit holds that its text does not show, hashed so that a long link
 * or a whole drawing weighs no more in the comparison than a few characters
 * of text.
 *
 * Attributes are read as written. The resolved `href` and `src` properties
 * depend on the document's address, which the live note and a parsed copy
 * of the previous version do not share.
 */
function signatureOf(element: Element): string {
  const scopes = [element, ...trailingNodes(element).filter(isElement)];
  const parts: string[] = [];
  for (const link of matchesWithin(scopes, 'a[href]')) {
    parts.push(`href=${link.getAttribute('href')}`);
  }
  for (const media of matchesWithin(scopes, '[data-media-id]')) {
    parts.push(
      `media=${media.getAttribute('data-media-id')}.${media.getAttribute('data-media-ext')}`
    );
  }
  for (const image of matchesWithin(scopes, 'img[src]')) {
    // A media figure's image is named by the figure, and its `src` by the runtime.
    if (!image.closest('[data-media-id]')) parts.push(`img=${image.getAttribute('src')}`);
  }
  for (const code of matchesWithin(scopes, 'pre > code[class]')) {
    parts.push(`code=${code.getAttribute('class')}`);
  }
  for (const box of matchesWithin(scopes, 'input[type="checkbox"]')) {
    parts.push(box.hasAttribute('checked') ? 'checked' : 'unchecked');
  }
  for (const svg of matchesWithin(scopes, '.markdown-diagram svg')) {
    if (!svg.parentElement?.closest('svg')) parts.push(`svg=${drawingOf(svg)}`);
  }
  return parts.length > 0 ? hash(parts.join('\u0002')) : '';
}

function kindOf(element: Element): string {
  const tag = element.tagName.toLowerCase();
  if (tag === 'pre') return 'code';
  if (tag === 'figure') {
    if (element.classList.contains('markdown-media')) return 'media';
    if (element.classList.contains('markdown-diagram')) return 'diagram';
  }
  return tag;
}

/**
 * The units of a rendered note, in order. `root` is the element holding the
 * rendered markdown.
 *
 * A block with no text and nothing embedded is not a unit: the empty
 * paragraphs the parser leaves around a media figure, a paragraph of line
 * breaks.
 */
export function extractNoteDiffUnits(root: Element): NoteDiffDomUnit[] {
  const units: NoteDiffDomUnit[] = [];
  const add = (element: HTMLElement, kind: string) => {
    const { text } = readNoteDiffUnitText(element);
    const embeds = element.matches(EMBEDDED) || element.querySelector(EMBEDDED) !== null;
    if (text.trim() === '' && !embeds) return;
    units.push({ element, kind, text, signature: signatureOf(element) });
  };

  for (const block of root.children) {
    if (!isElement(block) || isInline(block)) continue;
    const tag = block.tagName.toLowerCase();
    if (tag === 'ul' || tag === 'ol') {
      for (const item of block.children) {
        // The list's kind goes with the item: bullets turned into numbers are a change.
        if (isElement(item) && item.tagName.toLowerCase() === 'li') add(item, `${tag}-li`);
      }
    } else if (tag === 'table') {
      for (const row of block.querySelectorAll('tr')) {
        if (isElement(row) && row.closest('table') === block) add(row, 'tr');
      }
    } else {
      add(block, kindOf(block));
    }
  }
  return units;
}

/** How far down the page's layout an element's top is, by its offset parents. */
function layoutTop(element: HTMLElement): number {
  let top = 0;
  let node: HTMLElement | null = element;
  while (node) {
    top += node.offsetTop;
    const parent: Element | null = node.offsetParent;
    node = parent && isElement(parent) ? parent : null;
    // `offsetTop` is measured from inside the offset parent's border.
    if (node) top += node.clientTop;
  }
  return top;
}

/**
 * Each unit's box in `container`'s content: `top` is the distance from the
 * top of what `container` scrolls, so it does not change as it scrolls.
 *
 * Measured from layout offsets rather than `getBoundingClientRect`: the
 * note's dialog opens and closes with a scale animation, and rectangles
 * read during it are scaled with it.
 */
export function measureNoteDiffUnits(
  units: readonly NoteDiffDomUnit[],
  container: HTMLElement
): NoteDiffUnitBox[] {
  const origin = layoutTop(container) + container.clientTop;
  return units.map(({ element }) => ({
    top: layoutTop(element) - origin,
    height: element.offsetHeight,
  }));
}
