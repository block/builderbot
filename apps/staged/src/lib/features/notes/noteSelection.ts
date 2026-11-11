/**
 * Reading a reply-able quote out of a text selection in a rendered note: the
 * selected text, and the heading path it sits under (`Architecture › Data
 * flow`), taken from the note's rendered heading hierarchy at selection time.
 */

import { normalizeQuoteText } from '../sessions/composerDraft';

const HEADING_SELECTOR = 'h1, h2, h3, h4, h5, h6';

export type HeadingPathOptions = {
  /** Headings to leave out of every path (the note's title, which heads them all). */
  ignore?: (heading: Element) => boolean;
};

/** The node a range starts at: inside an element, the child at its offset. */
export function rangeStartNode(range: Range): Node {
  const container = range.startContainer;
  if (container.nodeType === Node.ELEMENT_NODE && container.childNodes.length > 0) {
    return (
      container.childNodes[range.startOffset] ??
      container.childNodes[container.childNodes.length - 1]
    );
  }
  return container;
}

function headingText(heading: Element): string {
  return (heading.textContent ?? '').replace(/\s+/g, ' ').trim();
}

/**
 * The headings `node` sits under within `root`, outermost first: for each
 * level, the nearest heading before it that a deeper or equal heading hasn't
 * closed. A node inside a heading sits under that heading too.
 */
export function headingPathFor(
  root: Element,
  node: Node,
  options: HeadingPathOptions = {}
): string[] {
  const stack: { level: number; text: string }[] = [];
  for (const heading of root.querySelectorAll(HEADING_SELECTOR)) {
    if (heading !== node) {
      const position = heading.compareDocumentPosition(node);
      const atOrAfter =
        position & Node.DOCUMENT_POSITION_FOLLOWING ||
        position & Node.DOCUMENT_POSITION_CONTAINED_BY;
      // Headings come in document order: the rest are past the node.
      if (!atOrAfter) break;
    }
    if (options.ignore?.(heading)) continue;
    const text = headingText(heading);
    if (!text) continue;
    const level = Number(heading.tagName.slice(1));
    while (stack.length > 0 && stack[stack.length - 1].level >= level) stack.pop();
    stack.push({ level, text });
  }
  return stack.map((heading) => heading.text);
}

export type NoteSelectionQuote = {
  /** The selected text, tidied for quoting. */
  quote: string;
  headings: string[];
  /** A copy of the selected range, which outlives the selection itself. */
  range: Range;
};

/**
 * The quote a selection makes, when it is a non-empty selection wholly
 * inside `root`. A selection reaching outside the note (into the dialog's
 * header, the chat pane) makes none.
 */
export function quoteFromSelection(
  selection: Selection | null,
  root: Element,
  options: HeadingPathOptions = {}
): NoteSelectionQuote | null {
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return null;
  const range = selection.getRangeAt(0);
  if (!root.contains(range.commonAncestorContainer)) return null;
  const quote = normalizeQuoteText(selection.toString());
  if (!quote) return null;
  return {
    quote,
    headings: headingPathFor(root, rangeStartNode(range), options),
    range: range.cloneRange(),
  };
}
