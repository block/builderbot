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
  const viewHeight = field.clientHeight;
  if (viewHeight === 0) return;
  const viewTop = field.getBoundingClientRect().top + field.clientTop;
  const viewBottom = viewTop + viewHeight;
  const box = input.getBoundingClientRect();
  let { top, bottom } = box;
  if (box.height > viewHeight) {
    const line = caretLineRectIn(input);
    if (!line) return;
    ({ top, bottom } = line);
  }
  if (top < viewTop) field.scrollTop -= viewTop - top;
  else if (bottom > viewBottom) field.scrollTop += bottom - viewBottom;
}
