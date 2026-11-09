// imeKeydown.ts — tell a keydown the IME is handling from one for the page.

/**
 * Whether `e` belongs to an IME or dictation composition (Enter committing
 * it, arrows picking a candidate, Escape cancelling it) rather than to the
 * page's own shortcuts. `isComposing` misses the committing Enter in Safari,
 * which fires `compositionend` before that keydown; the keydown still carries
 * the IME's keyCode 229.
 */
export function isImeKeydown(e: Pick<KeyboardEvent, 'isComposing' | 'keyCode'>): boolean {
  return e.isComposing || e.keyCode === 229;
}
