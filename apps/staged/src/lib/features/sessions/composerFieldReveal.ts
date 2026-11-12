// composerFieldReveal.ts — Keep the input being typed in within the reply
// composer's field.

import { caretLineRectIn } from './composerCaretNav';

/**
 * Scroll `field`, and only the field, by the least that brings `input` into
 * its view: the whole input when it fits, otherwise the caret's line, which
 * the input's top edge would say nothing about. Nothing above the field
 * moves — the transcript, a dialog, the page — where `scrollIntoView` would
 * take every scroll ancestor along (and on iOS fight the visual-viewport
 * pinning). With no caret to measure in a tall input the field is left as
 * it is; the browser reveals the caret as typing goes on.
 */
export function revealInField(field: HTMLElement, input: HTMLElement): void {
  const view = viewOf(field);
  if (!view) return;
  const box = input.getBoundingClientRect();
  let { top, bottom } = box;
  if (box.height > view.height) {
    const line = caretLineRectIn(input);
    if (!line) return;
    ({ top, bottom } = line);
  }
  scrollFieldTo(field, view, top, bottom);
}

/**
 * Scroll `field`, and only the field, by the least that shows `el` — a quote
 * block put back by Undo, say, which nobody is typing in: the whole of it
 * when it fits the view, else its top, where the block's breadcrumb and
 * excerpt say what came back.
 */
export function revealElementInField(field: HTMLElement, el: HTMLElement): void {
  const view = viewOf(field);
  if (!view) return;
  const box = el.getBoundingClientRect();
  const bottom = box.height > view.height ? box.top + view.height : box.bottom;
  scrollFieldTo(field, view, box.top, bottom);
}

interface FieldView {
  top: number;
  bottom: number;
  height: number;
}

/** The field's view in viewport coordinates, or `null` without a layout. */
function viewOf(field: HTMLElement): FieldView | null {
  const height = field.clientHeight;
  if (height === 0) return null;
  const top = field.getBoundingClientRect().top + field.clientTop;
  return { top, bottom: top + height, height };
}

/** Scroll the field by the least that brings the span from `top` to `bottom` into its view. */
function scrollFieldTo(field: HTMLElement, view: FieldView, top: number, bottom: number): void {
  if (top < view.top) field.scrollTop -= view.top - top;
  else if (bottom > view.bottom) field.scrollTop += bottom - view.bottom;
}
