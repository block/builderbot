import { tick } from 'svelte';

export interface FocusCaretOptions {
  /**
   * Leave every scroll ancestor where it is. For an input whose container
   * reveals it itself (the reply composer's field), so focusing an input
   * below the fold doesn't also scroll the dialog or the page to it.
   */
  preventScroll?: boolean;
}

/**
 * Focus a contenteditable element and place the cursor at the end of its content.
 * Synchronous variant — use when the DOM is already up-to-date (e.g. inside a
 * Svelte `$effect` that just re-rendered content).
 */
export function focusAtEndSync(el: HTMLElement | null, options?: FocusCaretOptions): void {
  if (!el) return;
  el.focus(options);
  placeCaret(el, false);
}

/**
 * Focus a contenteditable element and place the cursor at the end of its content.
 * Awaits a tick before focusing so that any pending Svelte re-renders complete first.
 * Use when the DOM may not yet reflect the latest reactive state.
 */
export async function focusAtEnd(el: HTMLElement | null): Promise<void> {
  await tick();
  focusAtEndSync(el);
}

/**
 * Focus a contenteditable element and place the cursor at the start of its
 * content. Synchronous, like `focusAtEndSync`.
 */
export function focusAtStartSync(el: HTMLElement | null, options?: FocusCaretOptions): void {
  if (!el) return;
  el.focus(options);
  placeCaret(el, true);
}

function placeCaret(el: HTMLElement, atStart: boolean): void {
  const sel = window.getSelection();
  if (!sel) return;
  const range = document.createRange();
  range.selectNodeContents(el);
  range.collapse(atStart);
  sel.removeAllRanges();
  sel.addRange(range);
}
