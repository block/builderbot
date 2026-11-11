// composerCaretNav.ts — Caret movement between the inputs of the reply
// composer's field.
//
// The field holds several contenteditable inputs in document order (the text
// before a quote, the quote's reply, …, the text after the last quote). To the
// user they read as one editor, so ArrowUp on the first line of one and
// ArrowDown on the last line of one move the caret into the neighbouring
// input, as they would across the lines of a single textarea.

/** Which edge lines of an input the caret is on. */
export interface CaretLineEdges {
  /** The caret is on the input's first line. */
  first: boolean;
  /** The caret is on the input's last line. */
  last: boolean;
}

/** The vertical extent of a line, in viewport coordinates. */
export interface LineRect {
  top: number;
  bottom: number;
}

/** A caret within this share of a line height of an edge counts as on that edge's line. */
const EDGE_TOLERANCE = 0.75;

/** The caret anchor the editor keeps after a badge; blank for every purpose here. */
const ZERO_WIDTH_SPACE = '​';

/**
 * Where the caret sits in `editor`'s lines, or `null` when there is no
 * collapsed caret in it or its line can't be told (a selection, or no box to
 * measure, as outside a layout engine).
 *
 * An empty input has one line, so its caret is on both edges, and a caret
 * with no content before it is on the first line, one with none after it on
 * the last, whatever the layout. Otherwise the caret's line is measured from
 * the content next to it — a collapsed range has no rect of its own in
 * WebKit or Firefox — and compared with the input's content box. At a soft
 * wrap the character before the caret ends one line and the one after starts
 * the next, and the DOM doesn't say which the caret is drawn on; the edge
 * check reads the one in the arrow's direction, so a caret at the wrap
 * crosses on the first press rather than needing a second.
 */
export function caretLineEdges(editor: HTMLElement): CaretLineEdges | null {
  const range = collapsedCaretIn(editor);
  if (!range) return null;
  if (isBlank(editor.textContent ?? '')) return { first: true, last: true };

  let first = !hasContent(editor, range, 'before');
  let last = !hasContent(editor, range, 'after');
  if (first && last) return { first, last };

  const box = contentBox(editor);
  if (!box) return null;
  if (!first) {
    const rect = caretLineRect(editor, range, 'before');
    if (!rect) return null;
    first = rect.top - box.top < box.tolerance;
  }
  if (!last) {
    const rect = caretLineRect(editor, range, 'after');
    if (!rect) return null;
    last = box.bottom - rect.bottom < box.tolerance;
  }
  return { first, last };
}

/**
 * The line the caret in `editor` is drawn on, in viewport coordinates, or
 * `null` without a collapsed caret in it or a way to measure one. At a soft
 * wrap this is the line the caret's next character would start.
 */
export function caretLineRectIn(editor: HTMLElement): LineRect | null {
  const range = collapsedCaretIn(editor);
  if (!range) return null;
  return caretLineRect(editor, range, 'after');
}

/** The selection's range when it is a collapsed caret inside `editor`. */
function collapsedCaretIn(editor: HTMLElement): Range | null {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0 || !selection.isCollapsed) return null;
  const range = selection.getRangeAt(0);
  return editor.contains(range.startContainer) ? range : null;
}

function isBlank(text: string): boolean {
  return text.replaceAll(ZERO_WIDTH_SPACE, '') === '';
}

/**
 * Whether there is anything — text, a badge, a line break — on `side` of the
 * caret within the editor.
 */
function hasContent(editor: HTMLElement, caret: Range, side: 'before' | 'after'): boolean {
  const range = document.createRange();
  if (side === 'before') {
    range.setStart(editor, 0);
    range.setEnd(caret.startContainer, caret.startOffset);
  } else {
    range.setStart(caret.startContainer, caret.startOffset);
    range.setEnd(editor, editor.childNodes.length);
  }
  return rangeHasContent(range);
}

function rangeHasContent(range: Range): boolean {
  if (!isBlank(range.toString())) return true;
  return range.cloneContents().querySelector('br') !== null;
}

/**
 * The line of the caret, measured from the content on `side` of it and
 * falling back to the other side at the content's edge, then to a marker put
 * in its place (only where the caret is between nodes, so nothing is split).
 */
function caretLineRect(
  editor: HTMLElement,
  caret: Range,
  side: 'before' | 'after'
): LineRect | null {
  const other = side === 'before' ? 'after' : 'before';
  return (
    neighbourRect(editor, caret, side) ?? neighbourRect(editor, caret, other) ?? markerRect(caret)
  );
}

/**
 * The line of the content next to the caret on `side`: the character there
 * (a one-character range has a rect where a collapsed one may not), or the
 * nearest sibling node with a box. A `<br>` before the caret ends the line
 * above it, so the caret is a line lower — unless nothing follows it, when
 * it is the trailing break a block renders no line for.
 */
function neighbourRect(
  editor: HTMLElement,
  caret: Range,
  side: 'before' | 'after'
): LineRect | null {
  const { startContainer: container, startOffset: offset } = caret;
  let node: Node | null;
  if (container instanceof Text) {
    const inside = side === 'before' ? offset > 0 : offset < container.data.length;
    if (inside) return charRect(container, side === 'before' ? offset - 1 : offset);
    node = side === 'before' ? container.previousSibling : container.nextSibling;
  } else {
    node = container.childNodes[side === 'before' ? offset - 1 : offset] ?? null;
  }
  while (node) {
    if (node instanceof Text) {
      if (node.data.length > 0) return charRect(node, side === 'before' ? node.data.length - 1 : 0);
    } else if (node instanceof Element) {
      const rect = node.getBoundingClientRect();
      if (rect.height > 0) {
        if (side === 'before' && node.tagName === 'BR' && hasContentAfterNode(editor, node)) {
          return { top: rect.bottom, bottom: rect.bottom + rect.height };
        }
        return { top: rect.top, bottom: rect.bottom };
      }
    }
    node = side === 'before' ? node.previousSibling : node.nextSibling;
  }
  return null;
}

function hasContentAfterNode(editor: HTMLElement, node: Node): boolean {
  const range = document.createRange();
  range.setStartAfter(node);
  range.setEnd(editor, editor.childNodes.length);
  return rangeHasContent(range);
}

/** The line of one character of `text`, preferring a rect with width (a wrapped space can leave one without). */
function charRect(text: Text, index: number): LineRect | null {
  const range = document.createRange();
  range.setStart(text, index);
  range.setEnd(text, index + 1);
  if (typeof range.getClientRects !== 'function') return null;
  const rects = [...range.getClientRects()].filter((rect) => rect.height > 0);
  const rect = rects.find((candidate) => candidate.width > 0) ?? rects[0];
  return rect ? { top: rect.top, bottom: rect.bottom } : null;
}

/**
 * Measure a zero-width marker at the caret and take it out again. Only
 * between nodes: inserting into a text node would split it, and the range
 * stays collapsed at the same point once the marker is gone.
 */
function markerRect(caret: Range): LineRect | null {
  if (!(caret.startContainer instanceof Element)) return null;
  const marker = document.createElement('span');
  marker.textContent = ZERO_WIDTH_SPACE;
  caret.insertNode(marker);
  const rect = marker.getBoundingClientRect();
  marker.remove();
  caret.collapse(true);
  return rect.height > 0 ? { top: rect.top, bottom: rect.bottom } : null;
}

/** The editor's content box (inside its padding) and the tolerance for a line at its edges. */
function contentBox(
  editor: HTMLElement
): { top: number; bottom: number; tolerance: number } | null {
  const box = editor.getBoundingClientRect();
  if (box.height === 0) return null;
  const style = getComputedStyle(editor);
  const fontSize = parseFloat(style.fontSize) || 13;
  const lineHeight = parseFloat(style.lineHeight) || fontSize * 1.5;
  return {
    top: box.top + (parseFloat(style.paddingTop) || 0),
    bottom: box.bottom - (parseFloat(style.paddingBottom) || 0),
    tolerance: lineHeight * EDGE_TOLERANCE,
  };
}

/**
 * The input the caret moves to from `current`, or `null` at the field's
 * ends. `editors` is in document order.
 */
export function adjacentEditor(
  editors: readonly HTMLElement[],
  current: HTMLElement,
  direction: 'up' | 'down'
): HTMLElement | null {
  const index = editors.indexOf(current);
  if (index === -1) return null;
  return editors[direction === 'up' ? index - 1 : index + 1] ?? null;
}

/** Whether an arrow keydown is one the field should consider for moving between inputs. */
export function isPlainVerticalArrow(
  event: Pick<KeyboardEvent, 'key' | 'shiftKey' | 'altKey' | 'ctrlKey' | 'metaKey'>
): event is KeyboardEvent & { key: 'ArrowUp' | 'ArrowDown' } {
  return (
    (event.key === 'ArrowUp' || event.key === 'ArrowDown') &&
    !event.shiftKey &&
    !event.altKey &&
    !event.ctrlKey &&
    !event.metaKey
  );
}
