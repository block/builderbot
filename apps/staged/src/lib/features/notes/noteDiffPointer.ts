/**
 * Telling a pointer that moved onto a change from a change that moved under
 * the pointer.
 *
 * What marks a note diff's changes scrolls: the markers with their text,
 * the connectors with either pane. When content moves under a pointer at
 * rest, the browser reports a move for it, at the place it already was, to
 * whatever is now under it. That is not a hover: a press scrolls the note,
 * and the change that arrived where the pointer is would take the light
 * from the one that was pressed.
 */
export class RestingPointer {
  /** Where the pointer last was in the window, as of the event before the one being handled. */
  private at: { x: number; y: number } | null = null;

  /**
   * Follow the pointer. Returns the undo.
   *
   * Listening on the window in the bubble phase: an element's own handlers
   * run first, and so compare an event with where the pointer was before it.
   */
  watch(): () => void {
    const track = (event: PointerEvent) => {
      this.at = { x: event.clientX, y: event.clientY };
    };
    window.addEventListener('pointermove', track, { passive: true });
    window.addEventListener('pointerdown', track, { passive: true });
    return () => {
      window.removeEventListener('pointermove', track);
      window.removeEventListener('pointerdown', track);
    };
  }

  /**
   * Whether a pointer event over a change is a hover: the pointer came to
   * it, and is one that can leave again.
   */
  hovers(event: PointerEvent): boolean {
    // A finger has no hover: a tap's would stay lit until the next tap.
    if (event.pointerType === 'touch') return false;
    return !(this.at && event.clientX === this.at.x && event.clientY === this.at.y);
  }
}
