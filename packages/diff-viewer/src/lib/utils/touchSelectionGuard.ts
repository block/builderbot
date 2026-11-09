// touchSelectionGuard.ts — which presses may start a line selection.
//
// The diff's lines select on `mousedown`. iOS and Android send a
// compatibility `mousedown` after a touch lifts, so a swipe over the after
// pane (a scroll, or a drag of the pane) started a line selection with a
// comment toolbar on whatever row the finger left. The guard follows the
// pointer events (which say what kind of press it was and whether it
// moved) and answers, when the mousedown comes, whether it was a deliberate
// tap: a touch or pen press that never left its spot by more than `slopPx`.
// A mouse press always may, and so does a mousedown with no pointer events
// before it (a browser without them).

export interface GuardedPointer {
  pointerType: string;
  clientX: number;
  clientY: number;
}

/** The distance a touch may wander and still count as a tap. */
export const TAP_SLOP_PX = 8;

export function createTouchSelectionGuard(slopPx = TAP_SLOP_PX) {
  let press: { pointerType: string; x: number; y: number; moved: boolean } | null = null;
  // A lifted touch's verdict, held for the compatibility mousedown that
  // follows it; null once consumed, or when the last press was a mouse.
  let liftedTap: boolean | null = null;

  return {
    pointerDown(event: GuardedPointer) {
      press = { pointerType: event.pointerType, x: event.clientX, y: event.clientY, moved: false };
      liftedTap = null;
    },
    pointerMove(event: GuardedPointer) {
      if (!press || press.moved) return;
      if (
        Math.abs(event.clientX - press.x) > slopPx ||
        Math.abs(event.clientY - press.y) > slopPx
      ) {
        press.moved = true;
      }
    },
    pointerEnd(event: GuardedPointer) {
      if (!press) return;
      liftedTap = event.pointerType === 'mouse' ? null : !press.moved;
      press = null;
    },
    /** The browser took the pointer: whatever it was, it was no tap. */
    pointerCancel() {
      liftedTap = press && press.pointerType !== 'mouse' ? false : null;
      press = null;
    },
    /**
     * Whether the `mousedown` arriving now may start a selection. A touch's
     * verdict is consumed, so a later mousedown without its own press (a
     * browser that sends none for a moved touch) gets the default.
     */
    allowsMouseDown(): boolean {
      if (press) return press.pointerType === 'mouse';
      if (liftedTap === null) return true;
      const allowed = liftedTap;
      liftedTap = null;
      return allowed;
    },
  };
}

export type TouchSelectionGuard = ReturnType<typeof createTouchSelectionGuard>;
