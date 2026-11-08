/**
 * Focus an element once the full-screen dialog it sits in has finished
 * sliding in.
 *
 * On iOS, focusing an input opens the keyboard and Safari scrolls the page
 * to reveal the input where it is at that moment. Mid-slide a full-screen
 * dialog is still mostly below the screen, so Safari scrolls the page by
 * nearly the whole screen and the dialog opens pushed off the top. Focusing
 * after the enter animation lets the input be revealed where it settles.
 *
 * Only a touch screen waits, since only an on-screen keyboard makes Safari
 * scroll. Anywhere else — a desktop dialog, full-screen in a narrow window
 * or not, a host that is not a dialog — the focus is immediate, as before,
 * so typing straight after opening keeps every keystroke.
 *
 * `focus` replaces the default `el.focus()` when the caller also selects or
 * places the caret. A deferred focus runs only if `el` is still in the
 * document and the dialog is still open: closing it mid-slide cancels the
 * enter animation, and focusing into the closing dialog would flash the
 * keyboard up.
 */
export function focusAfterDialogEnter<T extends HTMLElement>(
  el: T | null | undefined,
  focus: (el: T) => void = (target) => target.focus()
): void {
  if (!el) return;
  const dialog = hasCoarsePointer() ? el.closest<HTMLElement>('[data-full-screen]') : null;
  const entering = dialog?.getAnimations().filter((animation) => animation.playState === 'running');
  if (!dialog || !entering?.length) {
    focus(el);
    return;
  }
  // A cancelled animation rejects `finished`; allSettled waits it out too.
  void Promise.allSettled(entering.map((animation) => animation.finished)).then(() => {
    if (el.isConnected && dialog.dataset.state === 'open') focus(el);
  });
}

/**
 * Give a dialog's text input the focus it asks for as the dialog opens (or
 * as its content is swapped under it, e.g. a mode switch) — but on a touch
 * screen only if the input already has it.
 *
 * A focus that doesn't come from the user's own tap raises no keyboard on
 * iOS: the input is left focused with nothing to type on, showing its
 * spell-check underlines or selection handles, and it still takes a tap on
 * it to bring the keyboard up. With a mouse and keyboard the input is
 * focused (after focusAfterDialogEnter) so typing can start at once. An
 * input that already has focus is refocused through `focus` everywhere, so
 * a caller that places the caret or selects still gets that.
 */
export function autofocusOnOpen<T extends HTMLElement>(
  el: T | null | undefined,
  focus?: (el: T) => void
): void {
  if (!el) return;
  if (hasCoarsePointer() && el.ownerDocument.activeElement !== el) return;
  focusAfterDialogEnter(el, focus);
}

/**
 * The same query as `viewport.hasCoarsePointer`, read directly: a dialog's
 * form can ask for focus as it mounts, before anything has synced that.
 */
function hasCoarsePointer(): boolean {
  return window.matchMedia?.('(pointer: coarse)').matches ?? false;
}
