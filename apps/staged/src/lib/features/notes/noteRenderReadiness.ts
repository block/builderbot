/** The asynchronous inputs a note viewer's first render depends on. */
export interface NoteRenderInputs {
  /** Every embedded image's stored dimensions have been looked up (misses included). */
  mediaDimensionsKnown: boolean;
  /** The note has a pikchr fence, so its render calls the lazily loaded renderer. */
  needsPikchr: boolean;
  pikchrRendererLoaded: boolean;
  pikchrRendererLoadFailed: boolean;
}

/**
 * Whether a note viewer has everything its first `{@html}` render needs.
 *
 * `{@html}` replaces its whole subtree when the string changes, so rendering
 * before an async input is in and again once it lands rebuilds the note's DOM:
 * every image reloads, and every `<video>` is remounted — a clip the reader
 * has already started stops and goes back to the start, taking the pane's
 * scroll position with it. The viewer therefore renders nothing until each
 * input the markdown needs has answered. A failed pikchr load counts as
 * answered: the fence then renders its fallback once rather than never.
 */
export function noteRenderInputsReady(inputs: NoteRenderInputs): boolean {
  if (!inputs.mediaDimensionsKnown) return false;
  if (!inputs.needsPikchr) return true;
  return inputs.pikchrRendererLoaded || inputs.pikchrRendererLoadFailed;
}
