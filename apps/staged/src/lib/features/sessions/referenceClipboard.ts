/**
 * referenceClipboard.ts — Copy and paste that keep `#type:id` references
 *
 * A reference is shown as a badge whose visible text is the referenced item's
 * title, so the browser's own clipboard serialization loses the id: copying
 * `see #note:abc` out of a message and pasting it into the composer gave
 * `see My note`. Instead, every surface that shows badges writes the clipboard
 * itself, in two flavours:
 *
 * - `text/plain` is the source form (`see #note:abc`) — what the composer, the
 *   notes editor and anything outside the app read back.
 * - `text/html` is wrapped in `<div data-staged-copy>` with each badge as a
 *   `<span data-staged-ref="#note:abc">My note</span>`, so a rich-text target
 *   inside the app can tell this payload from foreign HTML and put the
 *   references back, while one outside it pastes the readable title.
 *
 * A badge keeps its identity only when all of its text is inside the
 * selection; a selection ending halfway through a title copies that much of
 * the title and nothing else.
 */
import { HASHTAG_TOKEN_RE, escapeHtml } from './hashtagItems';

/** Marks `text/html` this app wrote, so paste can tell its own payload from foreign HTML. */
export const STAGED_COPY_ATTR = 'data-staged-copy';
/** Carries a badge's `#type:id` through `text/html`. */
export const STAGED_REF_ATTR = 'data-staged-ref';

export type CopyFormat = 'text' | 'html';

const REFERENCE_RE = new RegExp(`^${HASHTAG_TOKEN_RE.source}$`);

/** Every element a reference badge can be: in this app's HTML, the composer, or a rendered view. */
const BADGE_SELECTOR = `[${STAGED_REF_ATTR}], [data-token], [data-hashtag-ref], [data-hashtag-id]`;

const BLOCK_TAGS = new Set([
  'p',
  'div',
  'li',
  'ul',
  'ol',
  'blockquote',
  'pre',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'table',
  'thead',
  'tbody',
  'tr',
  'hr',
  'section',
  'article',
]);
const CELL_TAGS = new Set(['td', 'th']);
/**
 * Markdown separates these with a blank line, as the browser's own `text/plain`
 * does; list items, table rows and layout containers are single lines.
 */
const MARKDOWN_BLOCK_TAGS = new Set([
  'p',
  'blockquote',
  'pre',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'ul',
  'ol',
  'table',
]);

/** The tags the HTML flavour keeps, so a rich target gets structure as well as text. */
const SEMANTIC_TAGS = new Set([
  'p',
  'strong',
  'b',
  'em',
  'i',
  's',
  'del',
  'code',
  'pre',
  'ul',
  'ol',
  'li',
  'blockquote',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'table',
  'thead',
  'tbody',
  'tr',
  'td',
  'th',
]);

const ZERO_WIDTH_SPACE = /\u200B/g;

/** Elements whose content the page never shows, so a copy must not show it either. */
const UNRENDERED_TAGS = new Set(['script', 'style', 'template', 'noscript']);

/**
 * The `#type:id` a badge stands for, or null for any other element.
 *
 * The DOM is read as untrusted: whatever the attribute holds has to be a
 * well-formed reference, or the element copies as its text.
 */
export function badgeReference(element: Element): string | null {
  const ref =
    element.getAttribute(STAGED_REF_ATTR) ??
    element.getAttribute('data-token') ??
    element.getAttribute('data-hashtag-ref') ??
    hashtagTypeIdReference(element);
  return ref !== null && REFERENCE_RE.test(ref) ? ref : null;
}

/** Non-interactive badges (timeline titles) carry only the resolved type and id. */
function hashtagTypeIdReference(element: Element): string | null {
  const type = element.getAttribute('data-hashtag-type');
  const id = element.getAttribute('data-hashtag-id');
  return type && id ? `#${type}:${id}` : null;
}

/** Write both flavours of `selection` to a copy or cut event's data. */
export function writeReferenceClipboard(data: DataTransfer, selection: Selection): void {
  data.setData('text/plain', serializeSelection(selection, 'text'));
  data.setData(
    'text/html',
    `<div ${STAGED_COPY_ATTR}>${serializeSelection(selection, 'html')}</div>`
  );
}

/**
 * The source text a paste carries: this app's own HTML read back with its
 * references, else whatever `text/plain` holds.
 */
export function readReferenceClipboard(data: DataTransfer): string {
  return stagedCopyText(data.getData('text/html')) ?? data.getData('text/plain');
}

/** The selection in one flavour; separate ranges (Firefox) read as lines. */
export function serializeSelection(selection: Selection, format: CopyFormat): string {
  const parts: string[] = [];
  for (let i = 0; i < selection.rangeCount; i++) {
    parts.push(serializeRange(selection.getRangeAt(i), format));
  }
  return parts.join(format === 'html' ? '<br>' : '\n');
}

export function serializeRange(range: Range, format: CopyFormat): string {
  // A selection inside one badge's title has a text node for its common
  // ancestor; start from the badge so a whole title still copies as the reference.
  const root = range.commonAncestorContainer;
  const badge = elementOf(root)?.closest(BADGE_SELECTOR);
  if (badge) return serializeNode(badge, format, range);
  // The common ancestor itself is only partly selected: its contents stand alone.
  return root.nodeType === Node.TEXT_NODE
    ? text(root as Text, format, range)
    : children(root, format, range);
}

/** Source text for this app's own `text/html` payload; other HTML is never read. */
export function stagedCopyText(html: string): string | undefined {
  const copy = ownCopy(html);
  return copy ? serializeNode(copy, 'text') : undefined;
}

/**
 * This app's own `text/html` payload with every reference back as `#type:id`
 * text, for a rich-text target that reads HTML; undefined for other HTML.
 */
export function stagedCopyHtml(html: string): string | undefined {
  const copy = ownCopy(html);
  if (!copy) return;
  for (const badge of copy.querySelectorAll(`[${STAGED_REF_ATTR}]`)) {
    badge.replaceWith(badgeReference(badge) ?? badge.textContent ?? '');
  }
  return copy.innerHTML;
}

/**
 * Copy from the rendered surfaces — messages, note bodies, timeline titles —
 * with references intact. Runs in the capture phase on the document so it
 * precedes the browser's own serialization, and steps aside unless the
 * selection takes in a badge, so anything else keeps the browser's richer
 * copy. Editors are left alone: the composer writes its own clipboard and the
 * notes editor holds references as text already.
 */
export function listenForReferenceCopy(): () => void {
  const onCopy = (event: ClipboardEvent) => {
    const selection = document.getSelection();
    if (event.defaultPrevented || !event.clipboardData || !selection || selection.isCollapsed) {
      return;
    }
    const ranges = Array.from({ length: selection.rangeCount }, (_, i) => selection.getRangeAt(i));
    if (!ranges.length || ranges.some(isEditable) || !ranges.some(holdsBadge)) return;
    writeReferenceClipboard(event.clipboardData, selection);
    event.preventDefault();
  };
  document.addEventListener('copy', onCopy, true);
  return () => document.removeEventListener('copy', onCopy, true);
}

function ownCopy(html: string): Element | null {
  if (!html.includes(STAGED_COPY_ATTR)) return null;
  return new DOMParser().parseFromString(html, 'text/html').querySelector(`[${STAGED_COPY_ATTR}]`);
}

function elementOf(node: Node): Element | null {
  return node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
}

function isEditable(range: Range): boolean {
  return !!elementOf(range.commonAncestorContainer)?.closest(
    '[contenteditable]:not([contenteditable="false"])'
  );
}

function holdsBadge(range: Range): boolean {
  const root = elementOf(range.commonAncestorContainer);
  if (!root) return false;
  if (root.closest(BADGE_SELECTOR)) return true;
  for (const badge of root.querySelectorAll(BADGE_SELECTOR)) {
    if (range.intersectsNode(badge)) return true;
  }
  return false;
}

/** Serialize the part of `node` inside `range` — all of it, without a range. */
function serializeNode(node: Node, format: CopyFormat, range?: Range): string {
  if (range && !range.intersectsNode(node)) return '';
  if (node.nodeType === Node.TEXT_NODE) return text(node as Text, format, range);
  if (node.nodeType !== Node.ELEMENT_NODE) return '';

  const element = node as Element;
  if (unrendered(element)) return '';
  const tag = element.tagName.toLowerCase();
  if (tag === 'br') return format === 'html' ? '<br>' : '\n';

  const ref = badgeReference(element);
  if (ref && (!range || whole(element, range))) {
    return reference(ref, element.textContent ?? '', format);
  }

  // A partly selected badge is its selected title text, less the glue between
  // the icon and the title.
  if (ref) return children(element, format, range).replace(/^\s+/, '');

  const inner = children(element, format, range);
  if (format === 'text') return inner;
  if (tag === 'a') {
    const href = element.getAttribute('href') ?? '';
    return /^https?:\/\//i.test(href) ? `<a href="${escapeHtml(href)}">${inner}</a>` : inner;
  }
  if (SEMANTIC_TAGS.has(tag)) return `<${tag}>${inner}</${tag}>`;
  return BLOCK_TAGS.has(tag) ? `<div>${inner}</div>` : inner;
}

/**
 * The walk reads the DOM, not the rendering, so what the browser's own copy
 * would leave out — script, style and template content, `[hidden]` elements —
 * has to be left out here.
 */
function unrendered(element: Element): boolean {
  return UNRENDERED_TAGS.has(element.tagName.toLowerCase()) || element.hasAttribute('hidden');
}

function reference(ref: string, label: string, format: CopyFormat): string {
  if (format === 'text') return ref;
  const title = label.replace(ZERO_WIDTH_SPACE, '').trim();
  return `<span ${STAGED_REF_ATTR}="${escapeHtml(ref)}">${escapeHtml(title)}</span>`;
}

/**
 * The children in order. The whitespace the markup puts between blocks is
 * layout, not content, and is dropped. The text flavour has no tags to carry
 * structure, so siblings are separated as `siblingSeparator` says; the HTML
 * flavour keeps its tags and needs nothing between them. A sibling that
 * serializes to nothing takes no separator and gives none, except a table
 * cell: it still holds its column, so the tab after it is kept and the
 * columns of the row line up.
 */
function children(parent: Node, format: CopyFormat, range?: Range): string {
  let out = '';
  let previousTag: string | undefined;
  for (const child of parent.childNodes) {
    if (isLayoutWhitespace(child)) continue;
    const part = serializeNode(child, format, range);
    if (!part && !keepsColumn(child, range)) continue;
    const tag =
      child.nodeType === Node.ELEMENT_NODE ? (child as Element).tagName.toLowerCase() : '';
    if (format === 'text' && previousTag !== undefined) {
      out += pad(out, part, siblingSeparator(previousTag, tag));
    }
    out += part;
    previousTag = tag;
  }
  return out;
}

/** A rendered table cell inside the selection, which keeps its column even when empty. */
function keepsColumn(node: Node, range?: Range): boolean {
  if (node.nodeType !== Node.ELEMENT_NODE) return false;
  const element = node as Element;
  if (!CELL_TAGS.has(element.tagName.toLowerCase()) || unrendered(element)) return false;
  return !range || range.intersectsNode(element);
}

/** What the text flavour puts between two siblings; text and inline elements have the empty tag. */
function siblingSeparator(previous: string, current: string): string {
  if (CELL_TAGS.has(previous) || CELL_TAGS.has(current)) return '\t';
  if (MARKDOWN_BLOCK_TAGS.has(previous) && MARKDOWN_BLOCK_TAGS.has(current)) return '\n\n';
  return BLOCK_TAGS.has(previous) || BLOCK_TAGS.has(current) ? '\n' : '';
}

/**
 * The part of `separator` the ends of `before` and `after` do not already
 * supply, so a trailing `<br>` or an empty line is not doubled up. A tab
 * follows the browser's own rule: every cell after the first takes one, even
 * after an empty cell (two tabs in a row), unless the row has given nothing
 * yet or ends in a newline — a leading empty cell starts no tab, and a cell
 * ending in `<br>` is followed straight by the next one's text.
 */
function pad(before: string, after: string, separator: string): string {
  if (!separator) return '';
  if (separator === '\t') return before && !before.endsWith('\n') ? '\t' : '';
  const have = (before.match(/\n*$/)?.[0].length ?? 0) + (after.match(/^\n*/)?.[0].length ?? 0);
  return '\n'.repeat(Math.max(0, separator.length - have));
}

/**
 * A whitespace-only text node beside a block element or a table cell, or alone
 * in a block: the newlines markup puts between blocks, and the ones `marked`
 * puts between the cells of a row, which the browser never renders.
 */
function isLayoutWhitespace(node: Node): boolean {
  if (node.nodeType !== Node.TEXT_NODE || !/^[\n\r\t ]*$/.test((node as Text).data)) return false;
  const parentTag = node.parentElement?.tagName.toLowerCase();
  if (parentTag === 'tr') return true;
  const besideBlock = (sibling: Node | null) => {
    if (sibling === null) return !!parentTag && BLOCK_TAGS.has(parentTag);
    if (sibling.nodeType !== Node.ELEMENT_NODE) return false;
    const tag = (sibling as Element).tagName.toLowerCase();
    return BLOCK_TAGS.has(tag) || CELL_TAGS.has(tag);
  };
  return besideBlock(node.previousSibling) || besideBlock(node.nextSibling);
}

function text(node: Text, format: CopyFormat, range?: Range): string {
  let value = node.data;
  if (range) {
    if (node === range.endContainer) value = value.slice(0, range.endOffset);
    if (node === range.startContainer) value = value.slice(range.startOffset);
  }
  value = value.replace(ZERO_WIDTH_SPACE, '');
  return format === 'html' ? escapeHtml(value) : value;
}

/**
 * A badge keeps its identity only when all of its text is inside the
 * selection. One with no text has nothing to select, so it never does: the
 * comparison below would otherwise hold for any range that merely touches it.
 */
function whole(element: Element, range: Range): boolean {
  const label = visible(element.textContent ?? '');
  if (!label) return false;
  const content = element.ownerDocument.createRange();
  content.selectNodeContents(element);
  const clipped = range.cloneRange();
  if (clipped.compareBoundaryPoints(Range.START_TO_START, content) < 0) {
    clipped.setStart(content.startContainer, content.startOffset);
  }
  if (clipped.compareBoundaryPoints(Range.END_TO_END, content) > 0) {
    clipped.setEnd(content.endContainer, content.endOffset);
  }
  return visible(clipped.toString()) === label;
}

function visible(value: string): string {
  return value.replace(/[\s\u200B]/g, '');
}
