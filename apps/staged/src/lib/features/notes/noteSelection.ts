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

// ---------------------------------------------------------------------------
// Where the Reply button goes
// ---------------------------------------------------------------------------

/** Width the button is kept clear of the container's edges by. */
export const REPLY_BUTTON_EDGE_GAP = 8;
/** Below the selection's last line, with a mouse. */
const MOUSE_BUTTON_GAP = 6;
/**
 * Below the selection's last line, by touch: iOS hangs the end handle's knob
 * about 12px under the baseline, and the button sits below that.
 */
const TOUCH_BUTTON_GAP = 18;
/**
 * By touch the button ends this far left of the selection's end, clear of
 * the end handle (its bar and knob) that iOS draws there. The callout menu
 * (Copy, Look Up…) sits above the selection, so below it is clear of that.
 */
const TOUCH_HANDLE_CLEARANCE = 28;

export type ReplyButtonBox = {
  /** The selection's last non-empty client rect. */
  end: Pick<DOMRectReadOnly, 'right' | 'bottom'>;
  /** The scroll container's client rect and scroll position. */
  container: Pick<DOMRectReadOnly, 'top' | 'left'> & {
    scrollTop: number;
    scrollLeft: number;
    clientWidth: number;
  };
  buttonWidth: number;
  /** A coarse pointer: see the touch constants above. */
  touch: boolean;
};

/**
 * The button's offset inside the scroll container: under the end of the
 * selection, centred on it with a mouse, or to the left of the end handle
 * by touch, and kept inside the container's width either way.
 */
export function replyButtonOffset(box: ReplyButtonBox): { top: number; left: number } {
  const { end, container, buttonWidth, touch } = box;
  const endX = end.right - container.left + container.scrollLeft;
  const wanted = touch ? endX - TOUCH_HANDLE_CLEARANCE - buttonWidth : endX - buttonWidth / 2;
  const maxLeft = Math.max(
    REPLY_BUTTON_EDGE_GAP,
    container.clientWidth - buttonWidth - REPLY_BUTTON_EDGE_GAP
  );
  return {
    top:
      end.bottom -
      container.top +
      container.scrollTop +
      (touch ? TOUCH_BUTTON_GAP : MOUSE_BUTTON_GAP),
    left: Math.min(Math.max(wanted, REPLY_BUTTON_EDGE_GAP), maxLeft),
  };
}

/**
 * Scroll `container` so the end of `range` is in view above a band of
 * `clearance` px laid over the container's bottom edge (a phone's docked
 * Reply button, which appears over the end of a selection made in the last
 * lines). The least scroll that does it, as `scrollIntoView({ block:
 * 'nearest' })` would for the end's rect: nothing when it is already in
 * view. Returns the distance scrolled.
 */
export function scrollRangeEndIntoView(
  container: HTMLElement,
  range: Range,
  clearance: number
): number {
  const rects = [...range.getClientRects()].filter((rect) => rect.width > 0 && rect.height > 0);
  const end = rects[rects.length - 1] ?? range.getBoundingClientRect();
  if (end.width === 0 && end.height === 0) return 0;
  const box = container.getBoundingClientRect();
  const visibleBottom = box.bottom - clearance;
  let delta = 0;
  if (end.bottom > visibleBottom) delta = end.bottom - visibleBottom;
  else if (end.top < box.top) delta = end.top - box.top;
  if (delta !== 0) container.scrollTop += delta;
  return delta;
}
