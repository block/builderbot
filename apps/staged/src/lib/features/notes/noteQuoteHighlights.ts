/**
 * Marking the passages a note's reply draft quotes, in the rendered note.
 *
 * Each quote is found in the note's text (noteQuoteLocation.ts) and the text
 * it covers is wrapped, a text node at a time, in
 * `<span class="note-quote-highlight">`: a passage running through bold
 * text, links, list items or table cells is several spans, one per piece of
 * text, so no element of the note is moved or split. Where quotes overlap,
 * a piece belongs to all of them (`data-quote-ids`) and is cut at each
 * quote's edge.
 *
 * A span that can be pressed to reopen its quote's reply names that quote in
 * `data-quote-id`: the shortest quote covering it, so a quote inside a
 * longer one stays reachable. Text that already does something when pressed
 * (a link, a hashtag badge) is marked but keeps its own press. One span per
 * quote, its first pressable one, is the quote's keyboard stop (`role`,
 * `tabindex`).
 *
 * The spans live in markup the note renders with `{@html}`, so they are
 * gone whenever the note is rendered again, and `applyQuoteHighlights` is
 * run again then. A quote whose text the note no longer holds gets no span.
 */

import type { DraftQuote } from '../sessions/composerDraft';
import {
  buildNoteTextIndex,
  locateQuote,
  positionInIndex,
  type NoteTextIndex,
  type NoteTextSpan,
} from './noteQuoteLocation';
import type { HeadingPathOptions } from './noteSelection';

export const QUOTE_HIGHLIGHT_CLASS = 'note-quote-highlight';
const HIGHLIGHT_SELECTOR = `span.${QUOTE_HIGHLIGHT_CLASS}`;

/** What a quote needs to be found and marked. */
export type HighlightableQuote = Pick<DraftQuote, 'id' | 'quote' | 'headings' | 'anchor'>;

/**
 * Text left unwrapped: an HTML span inside an SVG diagram is not rendered,
 * and the rest holds no text a reader sees. It still counts when the quote
 * is located, so a passage running across a diagram is marked on both sides.
 */
const UNWRAPPABLE = 'svg, script, style, textarea, select';

/** Text that already acts on a press, which a mark must not take over. */
const INTERACTIVE = 'a[href], button, summary, [role="button"], [data-hashtag-ref]';

/**
 * Elements that sit in a line of text. Whitespace between two of them (or
 * between one and text) is part of the line and is marked with the passage;
 * whitespace between anything else is markup indentation, where a span
 * would at best do nothing and inside a table or list would be a stray box.
 */
const INLINE_TAGS = new Set([
  'A',
  'ABBR',
  'B',
  'CITE',
  'CODE',
  'DEL',
  'EM',
  'I',
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

function isInline(node: Node | null): boolean {
  if (!node) return false;
  if (node.nodeType === Node.TEXT_NODE) return true;
  return node.nodeType === Node.ELEMENT_NODE && INLINE_TAGS.has((node as Element).tagName);
}

/** A stretch of one text node: `[from, to)`. */
type Piece = { node: Text; from: number; to: number };

/**
 * The text a located passage covers, a node at a time: from its first
 * character to its last, whole nodes in between, and the whitespace-only
 * nodes between pieces of the same line.
 */
function piecesOfSpan(index: NoteTextIndex, span: NoteTextSpan): Piece[] {
  const first = positionInIndex(index, span.start);
  const last = positionInIndex(index, span.end - 1);
  const pieces: Piece[] = [];
  const walker = index.root.ownerDocument.createTreeWalker(index.root, NodeFilter.SHOW_TEXT);
  for (let at = first.node; at <= last.node; at++) {
    const node = index.nodes[at];
    pieces.push({
      node,
      from: at === first.node ? first.offset : 0,
      to: at === last.node ? last.offset + 1 : node.length,
    });
    if (at === last.node) break;
    // Text nodes the index left out (all whitespace) before the next piece.
    walker.currentNode = node;
    const next = index.nodes[at + 1];
    for (let gap = walker.nextNode(); gap && gap !== next; gap = walker.nextNode()) {
      if (isInline(gap.previousSibling) && isInline(gap.nextSibling)) {
        pieces.push({ node: gap as Text, from: 0, to: (gap as Text).length });
      }
    }
  }
  return pieces;
}

type Located = { id: string; length: number; order: number; pieces: Piece[] };

/** The quote a press on a piece opens: the shortest covering it, the newest of equals. */
function primaryOf(covering: Located[]): Located {
  return covering.reduce((chosen, quote) =>
    quote.length < chosen.length || (quote.length === chosen.length && quote.order > chosen.order)
      ? quote
      : chosen
  );
}

function quoteIdsOf(span: Element): string[] {
  return (span.getAttribute('data-quote-ids') ?? '').split(' ').filter(Boolean);
}

function wrap(root: Element, text: Text, covering: Located[]) {
  const parent = text.parentNode;
  if (!parent) return;
  const span = root.ownerDocument.createElement('span');
  span.className = QUOTE_HIGHLIGHT_CLASS;
  if (covering.length > 1) span.classList.add('is-overlap');
  span.setAttribute('data-quote-ids', covering.map((quote) => quote.id).join(' '));
  const acting = parent instanceof Element ? parent.closest(INTERACTIVE) : null;
  if (!acting || !root.contains(acting)) {
    span.setAttribute('data-quote-id', primaryOf(covering).id);
    span.title = 'Edit your reply';
  }
  parent.insertBefore(span, text);
  span.appendChild(text);
}

/**
 * Mark every quote that can be found in the note rendered in `root`,
 * replacing whatever marks it had. Returns the ids of the quotes marked.
 */
export function applyQuoteHighlights(
  root: HTMLElement,
  quotes: readonly HighlightableQuote[],
  options: HeadingPathOptions = {}
): string[] {
  clearQuoteHighlights(root);
  if (quotes.length === 0) return [];

  const index = buildNoteTextIndex(root);
  const located: Located[] = [];
  quotes.forEach((quote, order) => {
    const span = locateQuote(index, quote, options);
    if (!span) return;
    located.push({
      id: quote.id,
      length: span.end - span.start,
      order,
      pieces: piecesOfSpan(index, span),
    });
  });

  const byNode = new Map<Text, { from: number; to: number; quote: Located }[]>();
  for (const quote of located) {
    for (const piece of quote.pieces) {
      if (piece.to <= piece.from) continue;
      const parent = piece.node.parentElement;
      if (!parent || parent.closest(UNWRAPPABLE)) continue;
      const covers = byNode.get(piece.node) ?? [];
      covers.push({ from: piece.from, to: piece.to, quote });
      byNode.set(piece.node, covers);
    }
  }

  const marked = new Set<string>();
  for (const [node, covers] of byNode) {
    // Cut at every quote's edge, and wrap from the end of the node back, so
    // each split leaves the offsets before it as they were.
    const cuts = [...new Set(covers.flatMap((cover) => [cover.from, cover.to]))].sort(
      (a, b) => a - b
    );
    for (let i = cuts.length - 2; i >= 0; i--) {
      const from = cuts[i];
      const to = cuts[i + 1];
      const covering = covers
        .filter((cover) => cover.from <= from && cover.to >= to)
        .map((cover) => cover.quote);
      if (covering.length === 0) continue;
      if (to < node.length) node.splitText(to);
      const text = from > 0 ? node.splitText(from) : node;
      wrap(root, text, covering);
      for (const quote of covering) marked.add(quote.id);
    }
  }

  // Each quote's first pressable span is its keyboard stop.
  const stopped = new Set<string>();
  for (const span of root.querySelectorAll<HTMLElement>(`${HIGHLIGHT_SELECTOR}[data-quote-id]`)) {
    const id = span.getAttribute('data-quote-id')!;
    if (stopped.has(id)) continue;
    stopped.add(id);
    span.setAttribute('role', 'button');
    span.tabIndex = 0;
  }

  return located.filter((quote) => marked.has(quote.id)).map((quote) => quote.id);
}

/**
 * Only non-empty text is merged back: an empty text node may be an anchor
 * Svelte keeps in the markup (see textHighlight.clearHighlights).
 */
function isNonEmptyText(node: Node | null): node is Text {
  return node !== null && node.nodeType === Node.TEXT_NODE && (node as Text).data.length > 0;
}

function joinText(first: Node | null, second: Node | null) {
  if (!isNonEmptyText(first) || !isNonEmptyText(second) || first.nextSibling !== second) return;
  first.data += second.data;
  second.remove();
}

/**
 * Take every mark out of `root`, leaving its text as it was: what each span
 * held goes back in its place, and the text split around it is joined up
 * again, so marks applied over and over don't leave the note in ever
 * smaller text nodes (a search of the note matches within one node).
 */
export function clearQuoteHighlights(root: HTMLElement): void {
  for (const span of root.querySelectorAll(HIGHLIGHT_SELECTOR)) {
    const before = span.previousSibling;
    const after = span.nextSibling;
    const children = [...span.childNodes];
    span.replaceWith(...children);
    if (children.length === 0) continue;
    joinText(children[children.length - 1], after);
    joinText(before, children[0]);
  }
}

/** The spans marking a quote, in document order. */
export function quoteHighlightElements(root: Element, quoteId: string): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(HIGHLIGHT_SELECTOR)].filter((span) =>
    quoteIdsOf(span).includes(quoteId)
  );
}

/** A quote's marked passage, first span to last; null when it has no mark. */
export function quoteHighlightRange(root: Element, quoteId: string): Range | null {
  const spans = quoteHighlightElements(root, quoteId);
  if (spans.length === 0) return null;
  const range = root.ownerDocument.createRange();
  range.setStartBefore(spans[0]);
  range.setEndAfter(spans[spans.length - 1]);
  return range;
}

/** The quote's keyboard stop, to return focus to; null when it has none. */
export function quoteHighlightStop(root: Element, quoteId: string): HTMLElement | null {
  return (
    [...root.querySelectorAll<HTMLElement>(`${HIGHLIGHT_SELECTOR}[role="button"]`)].find(
      (span) => span.getAttribute('data-quote-id') === quoteId
    ) ?? null
  );
}

/**
 * The quote a press on `target` reopens: that of the marked span it is in,
 * or null for unmarked text and for marked text with a press of its own.
 */
export function quoteIdAtTarget(root: Element, target: EventTarget | null): string | null {
  // `Element`, not `HTMLElement`: a find-in-note match inside a mark is still a press on it.
  const element =
    target instanceof Element ? target : target instanceof Node ? target.parentElement : null;
  const span = element?.closest(HIGHLIGHT_SELECTOR);
  if (!span || !root.contains(span)) return null;
  return span.getAttribute('data-quote-id');
}

/** Put `className` on the spans marking `quoteId`, and take it off every other. */
export function setQuoteHighlightState(
  root: Element,
  className: string,
  quoteId: string | null
): void {
  for (const span of root.querySelectorAll(HIGHLIGHT_SELECTOR)) {
    span.classList.toggle(className, quoteId !== null && quoteIdsOf(span).includes(quoteId));
  }
}
