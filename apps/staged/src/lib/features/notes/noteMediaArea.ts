/**
 * noteMediaArea.ts — the scroll-area height NoteModal's media cap follows.
 *
 * mediaStyles.css caps note images and videos at a share of the height of
 * the scroll area they sit in. Most hosts make that area a size container
 * and the cap reads it with `cqh`. NoteModal can't: with the chat pane closed
 * its dialog grows with the note up to a maximum height, and size containment
 * would leave the scroll area with no height of its own to grow from.
 *
 * Measuring the scroll area as it is would not work either, since a short
 * note's area is only as tall as its media and the cap would shrink with
 * itself. What the cap wants is the height the area has once the dialog
 * reaches its maximum: the area now plus whatever the dialog can still grow.
 * That stays put as the media inside resizes, and still follows the window,
 * the header and bottom bar, and the chat pane pinning the dialog's height.
 * It is measured inside the scroll box's padding, like `cqh`, and written to
 * the scroll box as `--media-area-height`.
 */

/** The custom property mediaStyles.css reads in place of `100cqh`. */
export const MEDIA_AREA_PROPERTY = '--media-area-height';

/** The selection dock's overlap NoteModal pads the scroll area's bottom by. */
const DOCK_OVERLAP_PROPERTY = '--selection-dock-overlap';

/** A running `trackNoteMediaArea`. */
export interface NoteMediaAreaTracker {
  /** Measure again on the next frame if a measurement was skipped. */
  refresh: () => void;
  /** Stop tracking and clear `--media-area-height`. */
  stop: () => void;
}

/**
 * The scroll area's height at the dialog's full height: its height now plus
 * the room the dialog still has under its `max-height` (`null` for none, as
 * in a full-screen dialog, whose height is fixed).
 */
export function noteMediaAreaHeight(
  scrollAreaHeight: number,
  dialogHeight: number,
  dialogMaxHeight: number | null
): number {
  const room = dialogMaxHeight === null ? 0 : Math.max(0, dialogMaxHeight - dialogHeight);
  return Math.max(0, Math.round(scrollAreaHeight + room));
}

/**
 * The scroll area's content height: `clientHeight` less its vertical
 * padding, since `100cqh` in the other hosts measures the container's content
 * box too and the same area should give the same cap in every host. `null`
 * while the area is hidden (no height to measure) or while the selection
 * dock pads its bottom (`--selection-dock-overlap`, see NoteModal): that
 * padding comes and goes with a text selection, and following it would
 * resize the note's media under the selection.
 */
function scrollAreaContentHeight(scrollArea: HTMLElement): number | null {
  const height = scrollArea.clientHeight;
  if (height === 0) return null;
  const style = getComputedStyle(scrollArea);
  if ((Number.parseFloat(style.getPropertyValue(DOCK_OVERLAP_PROPERTY)) || 0) > 0) return null;
  const padding = Number.parseFloat(style.paddingTop) + Number.parseFloat(style.paddingBottom);
  return Math.max(0, height - (Number.isFinite(padding) ? padding : 0));
}

/**
 * Keep `scrollArea`'s `--media-area-height` current while it is mounted in
 * `dialog`. Written once straight away, so the first paint is capped, then
 * on the next frame after either box or the window resizes (a write inside
 * the observer's own callback would resize what it observes in the same
 * frame). A hidden area (the phone's chat-only view) keeps its last value
 * rather than capping its media to nothing, and so does an area padded for
 * the selection dock (see `scrollAreaContentHeight`).
 *
 * A measurement skipped that way is owed, not dropped: `refresh()` takes it
 * on the next frame. Call it once the dock's overlap is back to 0. The dock
 * leaving doesn't always resize a box (in a full-screen dialog its overlap
 * can sit inside the safe-area padding), so a resize of the window or the
 * dialog while it showed would otherwise go unmeasured until the next one.
 * `refresh()` does nothing while no measurement is owed.
 */
export function trackNoteMediaArea(
  scrollArea: HTMLElement | null | undefined,
  dialog: HTMLElement | null | undefined
): NoteMediaAreaTracker | undefined {
  if (!scrollArea || !dialog || typeof ResizeObserver === 'undefined') return;
  let skipped = false;
  const update = () => {
    const height = scrollAreaContentHeight(scrollArea);
    skipped = height === null;
    if (height === null) return;
    const maxHeight = Number.parseFloat(getComputedStyle(dialog).maxHeight);
    const area = noteMediaAreaHeight(
      height,
      dialog.offsetHeight,
      Number.isFinite(maxHeight) ? maxHeight : null
    );
    scrollArea.style.setProperty(MEDIA_AREA_PROPERTY, `${area}px`);
  };
  let frame = 0;
  const schedule = () => {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      update();
    });
  };
  update();
  const observer = new ResizeObserver(schedule);
  observer.observe(scrollArea);
  observer.observe(dialog);
  window.addEventListener('resize', schedule);
  return {
    refresh: () => {
      if (skipped) schedule();
    },
    stop: () => {
      if (frame) cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener('resize', schedule);
      scrollArea.style.removeProperty(MEDIA_AREA_PROPERTY);
    },
  };
}
