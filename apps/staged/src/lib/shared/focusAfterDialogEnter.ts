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
 * Anywhere else — a centred desktop dialog, a host that is not a dialog —
 * the focus is immediate, as before, so typing straight after opening keeps
 * every keystroke.
 *
 * `focus` replaces the default `el.focus()` when the caller also selects or
 * places the caret; it runs only if `el` is still in the document.
 */
export function focusAfterDialogEnter<T extends HTMLElement>(
  el: T | null | undefined,
  focus: (el: T) => void = (target) => target.focus()
): void {
  if (!el) return;
  const entering = el
    .closest<HTMLElement>('[data-full-screen]')
    ?.getAnimations()
    .filter((animation) => animation.playState === 'running');
  if (!entering?.length) {
    focus(el);
    return;
  }
  // A cancelled animation rejects `finished`; the dialog is still there to
  // focus into unless it was removed.
  void Promise.allSettled(entering.map((animation) => animation.finished)).then(() => {
    if (el.isConnected) focus(el);
  });
}
