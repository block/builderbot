import { tick } from 'svelte';

/**
 * Focus a contenteditable element and place the cursor at the end of its content.
 * Synchronous variant — use when the DOM is already up-to-date (e.g. inside a
 * Svelte `$effect` that just re-rendered content).
 */
export function focusAtEndSync(el: HTMLElement | null): void {
  if (!el) return;
  el.focus();
  const sel = window.getSelection();
  if (sel) {
    const range = document.createRange();
    range.selectNodeContents(el);
    range.collapse(false);
    sel.removeAllRanges();
    sel.addRange(range);
  }
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
