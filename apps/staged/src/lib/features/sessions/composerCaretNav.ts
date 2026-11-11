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

/** A caret within this share of a line height of an edge counts as on that edge's line. */
const EDGE_TOLERANCE = 0.75;

/**
 * Where the caret sits in `editor`'s lines, or `null` when there is no
 * collapsed caret in it or its line can't be told (a selection, or no box to
 * measure, as outside a layout engine).
 *
 * An empty input has one line, so its caret is on both edges. Otherwise the
 * caret's own rect is compared with the input's content box; a caret at an
 * element boundary has no rect of its own, so a zero-width marker is measured
 * in its place and taken out again.
 */
export function caretLineEdges(editor: HTMLElement): CaretLineEdges | null {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0 || !selection.isCollapsed) return null;
  const range = selection.getRangeAt(0);
  if (!editor.contains(range.startContainer)) return null;
  if ((editor.textContent ?? '') === '') return { first: true, last: true };

  const caret = caretRect(range);
  if (!caret || caret.height === 0) return null;

  const style = getComputedStyle(editor);
  const fontSize = parseFloat(style.fontSize) || 13;
  const lineHeight = parseFloat(style.lineHeight) || fontSize * 1.5;
  const box = editor.getBoundingClientRect();
  if (box.height === 0) return null;
  const top = box.top + (parseFloat(style.paddingTop) || 0);
  const bottom = box.bottom - (parseFloat(style.paddingBottom) || 0);
  const tolerance = lineHeight * EDGE_TOLERANCE;
  return {
    first: caret.top - top < tolerance,
    last: bottom - caret.bottom < tolerance,
  };
}

function caretRect(range: Range): DOMRect | null {
  if (typeof range.getClientRects !== 'function') return null;
  const rects = range.getClientRects();
  if (rects.length > 0) return rects[0];
  // No rect: the caret is between elements (after a badge, or after a line's
  // <br>). Measure a marker there instead; the range stays collapsed at the
  // same point once the marker is gone, and nothing is split since the
  // container is an element, not a text node.
  if (!(range.startContainer instanceof Element)) return null;
  const marker = document.createElement('span');
  marker.textContent = '​';
  range.insertNode(marker);
  const rect = marker.getBoundingClientRect();
  marker.remove();
  range.collapse(true);
  return rect;
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
