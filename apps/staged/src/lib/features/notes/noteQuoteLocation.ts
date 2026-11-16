/**
 * Finding a quoted passage again in a rendered note, so the note can mark it
 * (see noteQuoteHighlights.ts).
 *
 * A reply draft keeps each quote as the text the reader selected. The note
 * it came from is rendered HTML, rebuilt whenever its markdown changes, so
 * there is no node or offset to keep: the passage is found by searching the
 * note's text for the quote's. Whitespace is left out of both sides of that
 * search. A selection read as text gains line breaks and tabs at block and
 * table-cell edges that the text nodes do not hold, and loses the
 * indentation between tags that they do, so only the other characters are
 * comparable.
 *
 * The same words can occur more than once, so a quote also carries where it
 * was selected (`QuoteAnchor`, captured here by `captureQuoteAnchor`): the
 * note's text on either side of it and how far into the note it began. Of
 * several occurrences the one whose surroundings match best is taken, then
 * one under the quote's headings, then the one nearest where the selection
 * began. A quote stored without an anchor has its headings to go by, and
 * otherwise its first occurrence.
 *
 * The surroundings also tell a passage that was rewritten away from the same
 * few words standing somewhere else: a short quote none of whose occurrences
 * has enough of its surroundings is not found at all (see
 * `CORROBORATED_QUOTE_LENGTH`). Two things stand in for surroundings an edit
 * beside the passage took away: the note's edge, for a passage that began or
 * ended near it, and the quote's headings, for words the note holds once.
 */

import type { DraftQuote, QuoteAnchor } from '../sessions/composerDraft';
import { headingPathFor, type HeadingPathOptions } from './noteSelection';

/** Characters of the note kept on each side of a selection. */
export const ANCHOR_CONTEXT_LENGTH = 32;

/**
 * With more occurrences than this still tied, their headings are not looked
 * up to tell them apart (each lookup walks the note's headings).
 */
const HEADING_CHECK_LIMIT = 64;

/**
 * A quote shorter than this (in squeezed characters, about six words) is
 * only found where the note still has some of the text that stood around it
 * (`MIN_SHARED_CONTEXT`). Where no occurrence does, the passage it was
 * selected from has been rewritten, and the words elsewhere are some other
 * "the cache": marking them would offer the reply from a passage it was
 * never written to. A quote this long found word for word is taken to be
 * the passage itself, moved or with new neighbours.
 */
export const CORROBORATED_QUOTE_LENGTH = ANCHOR_CONTEXT_LENGTH;

/**
 * How many characters of its anchor's surroundings, before and after
 * together, an occurrence of a short quote must share to be the passage
 * that was quoted. More than one: unrelated occurrences share a few by
 * chance (the "e" of a "the" before each, the first letter of the next
 * word), so one character proves nothing. Fewer than a side's worth: text
 * edited right up against the passage on one side leaves the other to go
 * by. An anchor holding less than this (the quote was nearly the whole
 * note) is asked for all it holds.
 *
 * The note's edge is surroundings too. A side of the anchor shorter than
 * `ANCHOR_CONTEXT_LENGTH` ran out at the note's start or end, and an
 * occurrence that far from the same edge, with all of that side matching,
 * counts as sharing a whole side (`sideScore`): a title is still the title
 * when the sentence after it is reworded.
 *
 * Failing all of that, an occurrence is still taken when it is the only one
 * in the note and stands under the quote's headings, of which there must be
 * some: the section says where the passage was, and with one place left in
 * the note holding its words there is nothing to mistake it for. With no
 * heading to place it by, a lone occurrence is no better founded than
 * several.
 */
export const MIN_SHARED_CONTEXT = 8;

const WHITESPACE = /\s/;

/** `text` with its whitespace left out: the form quotes and notes are compared in. */
export function squeezeText(text: string): string {
  return text.replace(/\s+/g, '');
}

/** A rendered note's text, squeezed, with the text nodes it came from. */
export type NoteTextIndex = {
  root: Element;
  /** Every text node's characters, in document order, whitespace left out. */
  text: string;
  /** The text nodes holding any of them. */
  nodes: Text[];
  /** Where each node's first character falls in `text`. */
  starts: number[];
  /** Each node's place in `nodes`. */
  nodeIndex: Map<Text, number>;
};

export function buildNoteTextIndex(root: Element): NoteTextIndex {
  const nodes: Text[] = [];
  const starts: number[] = [];
  const nodeIndex = new Map<Text, number>();
  const parts: string[] = [];
  let length = 0;
  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const squeezed = squeezeText((node as Text).data);
    if (!squeezed) continue;
    nodeIndex.set(node as Text, nodes.length);
    nodes.push(node as Text);
    starts.push(length);
    parts.push(squeezed);
    length += squeezed.length;
  }
  return { root, text: parts.join(''), nodes, starts, nodeIndex };
}

/** How many of the index's characters come before a DOM boundary point. */
function charactersBefore(index: NoteTextIndex, container: Node, offset: number): number {
  if (container.nodeType === Node.TEXT_NODE) {
    const at = index.nodeIndex.get(container as Text);
    if (at !== undefined) {
      return index.starts[at] + squeezeText((container as Text).data.slice(0, offset)).length;
    }
  }
  // Between nodes, or in a text node that is all whitespace: everything
  // before the first indexed node at or after the point. Such a node cannot
  // straddle the point, so "starts before it" sorts the nodes in two.
  const point = index.root.ownerDocument.createRange();
  point.setStart(container, offset);
  point.collapse(true);
  let low = 0;
  let high = index.nodes.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    if (point.comparePoint(index.nodes[middle], 0) < 0) low = middle + 1;
    else high = middle;
  }
  return low < index.nodes.length ? index.starts[low] : index.text.length;
}

/**
 * Where `range` sits in the note rendered in `root`, for a quote of its
 * text to carry (`quote` is that text as the selection read it). Null for a
 * range holding no text of the note's.
 */
export function captureQuoteAnchor(root: Element, range: Range, quote: string): QuoteAnchor | null {
  if (!root.contains(range.commonAncestorContainer)) return null;
  const index = buildNoteTextIndex(root);
  const start = charactersBefore(index, range.startContainer, range.startOffset);
  const end = charactersBefore(index, range.endContainer, range.endOffset);
  if (end <= start) return null;
  const anchor: QuoteAnchor = {
    prefix: index.text.slice(Math.max(0, start - ANCHOR_CONTEXT_LENGTH), start),
    suffix: index.text.slice(end, end + ANCHOR_CONTEXT_LENGTH),
    offset: start,
  };
  const text = index.text.slice(start, end);
  if (text !== squeezeText(quote)) anchor.text = text;
  return anchor;
}

/** A passage of the index's text: `[start, end)`. */
export type NoteTextSpan = { start: number; end: number };

function occurrences(text: string, needle: string): number[] {
  const found: number[] = [];
  if (!needle) return found;
  for (let at = text.indexOf(needle); at !== -1; at = text.indexOf(needle, at + 1)) found.push(at);
  return found;
}

/**
 * What one side of an occurrence counts for: the `shared` characters of the
 * `held` the anchor has on that side, or a whole side's worth when the anchor
 * ran out at the note's edge there and the occurrence, with `room` characters
 * of the note on that side, does too, behind the same text.
 */
function sideScore(shared: number, held: number, room: number): number {
  const atSameEdge = held < ANCHOR_CONTEXT_LENGTH && shared === held && room === held;
  return atSameEdge ? ANCHOR_CONTEXT_LENGTH : shared;
}

/** How much of an anchor's surroundings an occurrence at `[start, end)` shares. */
function contextScore(text: string, start: number, end: number, anchor: QuoteAnchor): number {
  let before = 0;
  while (
    before < anchor.prefix.length &&
    before < start &&
    text[start - 1 - before] === anchor.prefix[anchor.prefix.length - 1 - before]
  ) {
    before++;
  }
  let after = 0;
  while (
    after < anchor.suffix.length &&
    end + after < text.length &&
    text[end + after] === anchor.suffix[after]
  ) {
    after++;
  }
  return (
    sideScore(before, anchor.prefix.length, start) +
    sideScore(after, anchor.suffix.length, text.length - end)
  );
}

/** The candidates scoring highest, in their original order. */
function best(candidates: number[], score: (candidate: number) => number): number[] {
  let top = -Infinity;
  let kept: number[] = [];
  for (const candidate of candidates) {
    const value = score(candidate);
    if (value > top) {
      top = value;
      kept = [candidate];
    } else if (value === top) {
      kept.push(candidate);
    }
  }
  return kept;
}

function sameHeadings(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((heading, i) => heading === b[i]);
}

/** The text node holding the index's character `at`, and the character's offset in it. */
export function positionInIndex(
  index: NoteTextIndex,
  at: number
): { node: number; offset: number } {
  let low = 0;
  let high = index.nodes.length - 1;
  while (low < high) {
    const middle = (low + high + 1) >> 1;
    if (index.starts[middle] <= at) low = middle;
    else high = middle - 1;
  }
  const data = index.nodes[low].data;
  let remaining = at - index.starts[low];
  for (let offset = 0; offset < data.length; offset++) {
    if (WHITESPACE.test(data[offset])) continue;
    if (remaining === 0) return { node: low, offset };
    remaining--;
  }
  return { node: low, offset: data.length };
}

/**
 * Where a quote's passage is in the indexed note; null when its text is no
 * longer there (the note was rewritten), or is a short quote's and only
 * stands where too little of the quote's surroundings does, which leaves
 * the quote unmarked and otherwise as it was.
 */
export function locateQuote(
  index: NoteTextIndex,
  quote: Pick<DraftQuote, 'quote' | 'headings' | 'anchor'>,
  options: HeadingPathOptions = {}
): NoteTextSpan | null {
  const anchor = quote.anchor;
  // The passage as the text nodes spelled it, where the anchor kept that;
  // failing it (the note changed there), as the selection read it.
  const needles = [anchor?.text, squeezeText(quote.quote)].filter(
    (needle, i, all): needle is string => !!needle && all.indexOf(needle) === i
  );
  const underHeadings = (start: number) =>
    sameHeadings(
      headingPathFor(index.root, index.nodes[positionInIndex(index, start).node], options),
      quote.headings
    );
  for (const needle of needles) {
    const found = occurrences(index.text, needle);
    if (found.length === 0) continue;
    let candidates = found;
    if (anchor) {
      const score = (start: number) =>
        contextScore(index.text, start, start + needle.length, anchor);
      candidates = best(found, score);
      const needed = Math.min(MIN_SHARED_CONTEXT, anchor.prefix.length + anchor.suffix.length);
      if (needle.length < CORROBORATED_QUOTE_LENGTH && score(candidates[0]) < needed) {
        // Not the passage that was quoted, wherever the occurrences sit,
        // unless there is one only and it is in the quote's section (see
        // `MIN_SHARED_CONTEXT`).
        const sole = found.length === 1 && quote.headings.length > 0 && underHeadings(found[0]);
        if (!sole) continue;
      }
    }
    if (candidates.length > 1 && candidates.length <= HEADING_CHECK_LIMIT) {
      const under = candidates.filter(underHeadings);
      if (under.length > 0) candidates = under;
    }
    if (candidates.length > 1 && anchor) {
      candidates = best(candidates, (start) => -Math.abs(start - anchor.offset));
    }
    return { start: candidates[0], end: candidates[0] + needle.length };
  }
  return null;
}

/**
 * The quote, of `quotes`, whose passage in the note rendered in `root` is
 * exactly the one `passage` was just selected at: the same first and last
 * character, as the quotes are located now rather than by the offsets they
 * were stored with, which an edit earlier in the note moves. `passage`'s
 * anchor must be fresh (`captureQuoteAnchor` against `root` as it is). The
 * newest of several, the one a press on their shared mark opens; null when
 * no quote is there, or the passage has no anchor.
 */
export function quoteAtPassage<Q extends Pick<DraftQuote, 'quote' | 'headings' | 'anchor'>>(
  root: Element,
  quotes: readonly Q[],
  passage: Pick<DraftQuote, 'quote' | 'anchor'>,
  options: HeadingPathOptions = {}
): Q | null {
  const anchor = passage.anchor;
  if (!anchor || quotes.length === 0) return null;
  const start = anchor.offset;
  const end = start + (anchor.text ?? squeezeText(passage.quote)).length;
  const index = buildNoteTextIndex(root);
  let found: Q | null = null;
  for (const quote of quotes) {
    const span = locateQuote(index, quote, options);
    if (span && span.start === start && span.end === end) found = quote;
  }
  return found;
}
