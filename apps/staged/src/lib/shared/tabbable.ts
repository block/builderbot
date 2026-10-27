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

/**
 * The first element in document order that Tab would land on after
 * `boundary` and its subtree — wrapping to the document's first tabbable
 * outside the boundary when nothing follows it — or null when the boundary
 * holds the only tabbable content.
 */
export function firstTabbableAfter(boundary: Element): HTMLElement | null {
  let wrap: HTMLElement | null = null;
  for (const el of document.querySelectorAll<HTMLElement>(TABBABLE_SELECTOR)) {
    if (el === boundary || boundary.contains(el)) continue;
    if (!isTabbable(el)) continue;
    if (boundary.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING) return el;
    wrap ??= el;
  }
  return wrap;
}

/**
 * The first element in document order that Tab would land on inside `root`,
 * or null when it holds nothing tabbable.
 */
export function firstTabbableWithin(root: Element): HTMLElement | null {
  for (const el of root.querySelectorAll<HTMLElement>(TABBABLE_SELECTOR)) {
    if (isTabbable(el)) return el;
  }
  return null;
}

// `[aria-hidden="true"]` subtrees are the covered views (the parked workspace,
// an inactive WorkspaceLayer); their controls are not tab stops to hand focus
// to, even though — unlike `inert` — the attribute doesn't take them out of
// the browser's tab order.
function isTabbable(el: HTMLElement): boolean {
  if (
    el.tabIndex < 0 ||
    el.matches(':disabled') ||
    el.closest('[inert]') ||
    el.closest('[aria-hidden="true"]')
  )
    return false;
  return el.checkVisibility?.() ?? true;
}
