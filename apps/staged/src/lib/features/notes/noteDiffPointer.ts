/**
 * Telling a pointer that moved onto a change from a change that moved under
 * the pointer, and letting go of a hover whose element is gone.
 *
 * What marks a note diff's changes scrolls: the markers with their text,
 * the connectors with either pane. When content moves under a pointer at
 * rest, the browser reports a move for it, at the place it already was, to
 * whatever is now under it. That is not a hover: a press scrolls the note,
 * and the change that arrived where the pointer is would take the light
 * from the one that was pressed.
 */
import type { NoteDiffState } from './noteDiffState.svelte';

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
    // Not known to have moved until it has been seen somewhere else. The
    // watch starts with whoever draws the changes, which can be after the
    // pointer came to rest: the connectors appear when a key opens the
    // previous version, under a pointer that has not been seen since. The
    // move reported for it then is the first one seen, and is no hover. One
    // that really arrives goes on moving, and its next move is. From then
    // on the watch must not lapse while the pointer can still move: a move
    // it missed leaves `at` behind the pointer.
    if (!this.at) return false;
    return event.clientX !== this.at.x || event.clientY !== this.at.y;
  }
}

/**
 * The hover one set of markers or bands gave a change, kept so that it can
 * be taken back.
 *
 * A hover ends when the pointer leaves, or focus does. Neither is reported
 * for an element that is removed: a band scrolled out of the gutter, or the
 * previous version's pane closed with the pointer on one of its markers.
 * The change would stay lit with nothing pointing at it, and over a
 * selected change, until something else was hovered and left. So whoever
 * draws the elements says when the hovered one is no longer drawn (`keep`),
 * and when it stops drawing altogether (`release`).
 */
export class ChangeHover {
  /** The change lit from here, until it is left. */
  private lit: string | null = null;

  /** The pointer came to a change's element, or focus did. */
  enter(noteDiff: NoteDiffState, hunkId: string): void {
    this.lit = hunkId;
    noteDiff.hover(hunkId);
  }

  /** The pointer left a change's element, or focus did. */
  leave(noteDiff: NoteDiffState, hunkId: string): void {
    if (this.lit === hunkId) this.lit = null;
    if (noteDiff.hoveredHunkId === hunkId) noteDiff.hover(null);
  }

  /** The elements drawn have changed: a hover on one that is gone has ended. */
  keep(noteDiff: NoteDiffState, drawn: (hunkId: string) => boolean): void {
    if (this.lit !== null && !drawn(this.lit)) this.leave(noteDiff, this.lit);
  }

  /** Nothing is drawn any more. */
  release(noteDiff: NoteDiffState): void {
    if (this.lit !== null) this.leave(noteDiff, this.lit);
  }
}
