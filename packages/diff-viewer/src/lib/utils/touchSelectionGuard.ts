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
//
// The press starts from the viewer's own `pointerdown`, but the rest of the
// gesture is read from the window: once an ancestor takes pointer capture
// (DiffModal's container does, for a pane drag or a horizontal scroll, after
// its dead zone) the later pointermove/pointerup events are retargeted to it
// and no longer pass through the viewer, so listeners there would leave the
// press open with no end. Window listeners in the capture phase see every
// event of the gesture whatever element it is sent to. `lostpointercapture`
// without a pointerup (the capturing element went away) ends the press as a
// cancel.

export interface GuardedPointer {
  pointerType: string;
  clientX: number;
  clientY: number;
  /** Events of another pointer (a second finger) are not this press's. */
  pointerId?: number;
}

/** The `addEventListener`/`removeEventListener` pair of the window. */
export interface GuardEventSource {
  addEventListener(type: string, listener: (event: Event) => void, capture?: boolean): void;
  removeEventListener(type: string, listener: (event: Event) => void, capture?: boolean): void;
}

/** The distance a touch may wander and still count as a tap. */
export const TAP_SLOP_PX = 8;

export function createTouchSelectionGuard(
  slopPx = TAP_SLOP_PX,
  source: GuardEventSource | null = typeof window !== 'undefined' ? window : null
) {
  let press: {
    pointerType: string;
    pointerId: number | undefined;
    x: number;
    y: number;
    moved: boolean;
  } | null = null;
  // A lifted touch's verdict, held for the compatibility mousedown that
  // follows it; null once consumed, or when the last press was a mouse.
  let liftedTap: boolean | null = null;
  let listening = false;

  function isThisPress(event: GuardedPointer): boolean {
    return (
      press !== null &&
      (press.pointerId === undefined ||
        event.pointerId === undefined ||
        press.pointerId === event.pointerId)
    );
  }

  const onMove = (event: Event) => pointerMove(event as unknown as GuardedPointer);
  const onEnd = (event: Event) => pointerEnd(event as unknown as GuardedPointer);
  const onCancel = (event: Event) => {
    if (isThisPress(event as unknown as GuardedPointer)) pointerCancel();
  };

  function listen() {
    if (listening || !source) return;
    listening = true;
    source.addEventListener('pointermove', onMove, true);
    source.addEventListener('pointerup', onEnd, true);
    source.addEventListener('pointercancel', onCancel, true);
    source.addEventListener('lostpointercapture', onCancel, true);
  }

  function unlisten() {
    if (!listening || !source) return;
    listening = false;
    source.removeEventListener('pointermove', onMove, true);
    source.removeEventListener('pointerup', onEnd, true);
    source.removeEventListener('pointercancel', onCancel, true);
    source.removeEventListener('lostpointercapture', onCancel, true);
  }

  function pointerMove(event: GuardedPointer) {
    if (!press || press.moved || !isThisPress(event)) return;
    if (Math.abs(event.clientX - press.x) > slopPx || Math.abs(event.clientY - press.y) > slopPx) {
      press.moved = true;
    }
  }

  function pointerEnd(event: GuardedPointer) {
    if (!press || !isThisPress(event)) return;
    liftedTap = event.pointerType === 'mouse' ? null : !press.moved;
    press = null;
    unlisten();
  }

  /** The browser took the pointer: whatever it was, it was no tap. */
  function pointerCancel() {
    if (!press) return;
    liftedTap = press.pointerType !== 'mouse' ? false : null;
    press = null;
    unlisten();
  }

  return {
    /**
     * A press began on the viewer. From here the guard follows the pointer
     * through the window (see the header) until it lifts or is cancelled.
     */
    pointerDown(event: GuardedPointer) {
      press = {
        pointerType: event.pointerType,
        pointerId: event.pointerId,
        x: event.clientX,
        y: event.clientY,
        moved: false,
      };
      liftedTap = null;
      listen();
    },
    pointerMove,
    pointerEnd,
    pointerCancel,
    /**
     * Whether the `mousedown` arriving now may start a selection: the
     * verdict of the touch or pen press that just lifted (its compatibility
     * mousedown follows the pointerup), consumed so a later mousedown with
     * no press of its own (a browser that sends none for a moved touch)
     * gets the default. A mouse press, or no press, is allowed. A touch
     * still down has not lifted and so cannot have been a tap yet.
     */
    allowsMouseDown(): boolean {
      if (press) return press.pointerType === 'mouse';
      if (liftedTap === null) return true;
      const allowed = liftedTap;
      liftedTap = null;
      return allowed;
    },
    /** Drop the window listeners of a press in progress. */
    destroy() {
      press = null;
      liftedTap = null;
      unlisten();
    },
  };
}

export type TouchSelectionGuard = ReturnType<typeof createTouchSelectionGuard>;
