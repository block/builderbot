// touchTapGuard.ts — ignore the click a touch ends in when the finger travelled.
//
// iOS Safari turns a quick flick into a tap when it sees too few touch
// samples to start a scroll (60–200 px of travel, finger up within about
// 100 ms): no scroll happens, and a `click` lands on whatever was under the
// finger when it came down. On a list of cards that opens one. This is
// WebKit's tap synthesis, not an app handler, so it is undone in one place
// for every click target rather than per card.
//
// Distance is measured with pointer events. For such a flick `touchend`
// reports the position the touch started at, while `pointerup` reports where
// the finger actually lifted.

/** Travel between finger down and finger up beyond which a touch is not a tap. */
export const TAP_MAX_TRAVEL_PX = 10;

/** How long after a travelled touch ends its click can still arrive. */
export const TAP_CLICK_WINDOW_MS = 700;

/** The parts of a pointer event the guard reads. */
type PointerLike = Pick<
  PointerEvent,
  'pointerId' | 'pointerType' | 'isPrimary' | 'clientX' | 'clientY' | 'timeStamp'
>;

/** The parts of a click event the guard reads and acts on. */
type ClickLike = Pick<
  MouseEvent,
  'isTrusted' | 'timeStamp' | 'preventDefault' | 'stopImmediatePropagation'
>;

/**
 * Swallow the click that follows a primary touch lifted more than
 * `TAP_MAX_TRAVEL_PX` from where it landed. Mouse and pen input, plain taps
 * and scripted clicks pass through untouched. Returns the cleanup.
 */
export function guardTouchTaps(target?: EventTarget): () => void {
  const root = target ?? (typeof document === 'undefined' ? undefined : document);
  if (!root) return () => {};

  /** The touch being followed, from finger down to finger up. */
  let press: { pointerId: number; x: number; y: number } | null = null;
  /** `timeStamp` of the travelled touch's `pointerup` while its click is awaited. */
  let travelledAt: number | null = null;

  const onPointerDown = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    // Any new press starts over: a click still owed to the last touch never came.
    travelledAt = null;
    press =
      pointer.pointerType === 'touch' && pointer.isPrimary
        ? { pointerId: pointer.pointerId, x: pointer.clientX, y: pointer.clientY }
        : null;
  };

  const onPointerUp = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    if (!press || pointer.pointerId !== press.pointerId) return;
    const travel = Math.hypot(pointer.clientX - press.x, pointer.clientY - press.y);
    press = null;
    if (travel > TAP_MAX_TRAVEL_PX) travelledAt = pointer.timeStamp;
  };

  // The browser took the gesture (a scroll, usually) and sends no click.
  const onPointerCancel = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    if (press && pointer.pointerId === press.pointerId) press = null;
  };

  const onClick = (event: Event) => {
    if (travelledAt === null) return;
    const click = event as unknown as ClickLike;
    // A click made by script (`element.click()`) is not the touch's own.
    if (!click.isTrusted) return;
    const elapsed = click.timeStamp - travelledAt;
    travelledAt = null;
    if (elapsed > TAP_CLICK_WINDOW_MS) return;
    click.preventDefault();
    click.stopImmediatePropagation();
  };

  // Capture phase: ahead of every handler in the app, and unaffected by
  // components that stop propagation.
  root.addEventListener('pointerdown', onPointerDown, true);
  root.addEventListener('pointerup', onPointerUp, true);
  root.addEventListener('pointercancel', onPointerCancel, true);
  root.addEventListener('click', onClick, true);
  return () => {
    root.removeEventListener('pointerdown', onPointerDown, true);
    root.removeEventListener('pointerup', onPointerUp, true);
    root.removeEventListener('pointercancel', onPointerCancel, true);
    root.removeEventListener('click', onClick, true);
  };
}
