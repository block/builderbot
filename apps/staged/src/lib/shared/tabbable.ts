const TABBABLE_SELECTOR =
  'a[href], button, input, select, textarea, [tabindex], [contenteditable="true"]';

/**
 * The last element in document order that Shift+Tab could land on before
 * `boundary`, or null when nothing tabbable precedes it.
 */
export function lastTabbableBefore(boundary: Element): HTMLElement | null {
  let last: HTMLElement | null = null;
  for (const el of document.querySelectorAll<HTMLElement>(TABBABLE_SELECTOR)) {
    // querySelectorAll yields document order, so the first candidate that is
    // not before the boundary ends the walk.
    if (!(boundary.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_PRECEDING)) break;
    if (isTabbable(el)) last = el;
  }
  return last;
}

function isTabbable(el: HTMLElement): boolean {
  if (el.tabIndex < 0 || el.matches(':disabled') || el.closest('[inert]')) return false;
  return el.checkVisibility?.() ?? true;
}
