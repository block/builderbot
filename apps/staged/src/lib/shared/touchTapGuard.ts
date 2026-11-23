// touchTapGuard.ts — ignore the click a flick ends in.
//
// iOS Safari turns a quick flick into a tap when it sees too few touch
// samples to start a scroll (60–200 px of travel, finger up within about
// 100 ms): no scroll happens, and a `click` lands on whatever was under the
// finger when it came down. On a list of cards that opens one. This is
// WebKit's tap synthesis, not an app handler, so it is undone in one place
// for every click target rather than per card.
//
// A flick is the one touch that is both far and fast, so the guard asks for
// both. Neither is enough alone:
//
// - an ordinary tap is as short as a flick, and a thumb rolls 10–30 px as
//   it presses. iOS still clicks for that wherever nothing scrolls along
//   the roll (a toolbar or dialog button, a short menu, across a vertical
//   list), so a small distance limit drops real taps
// - a slow press has given iOS the samples to judge it: where it clicks,
//   it meant to, however far the finger rolled
//
// Distance is measured with pointer events. For such a flick `touchend`
// reports the position the touch started at, while `pointerup` reports where
// the finger actually lifted.

/**
 * Travel between finger down and finger up within which a touch is a tap,
 * however quick. A third of the shortest flick the audit saw turned into a
 * tap (60 px); a press that rolls further still passes when it is slow.
 */
export const TAP_MAX_TRAVEL_PX = 20;

/**
 * How soon the finger must lift for travel past `TAP_MAX_TRAVEL_PX` to be a
 * flick. The flicks WebKit turned into taps lifted within 100 ms; given
 * more samples it scrolled instead, and sent no click.
 */
export const FLICK_MAX_DURATION_MS = 200;

/** How long after a flick ends its click can still arrive. */
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
 * `TAP_MAX_TRAVEL_PX` from where it landed, within `FLICK_MAX_DURATION_MS`
 * of landing. Mouse and pen input, taps, slow presses and scripted clicks
 * pass through untouched. Returns the cleanup.
 */
export function guardTouchTaps(target?: EventTarget): () => void {
  const root = target ?? (typeof document === 'undefined' ? undefined : document);
  if (!root) return () => {};

  /** The touch being followed, from finger down to finger up. */
  let press: { pointerId: number; x: number; y: number; at: number } | null = null;
  /** `timeStamp` of the flick's `pointerup` while its click is awaited. */
  let flickedAt: number | null = null;

  const onPointerDown = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    // Any new press starts over: a click still owed to the last touch never came.
    flickedAt = null;
    press =
      pointer.pointerType === 'touch' && pointer.isPrimary
        ? {
            pointerId: pointer.pointerId,
            x: pointer.clientX,
            y: pointer.clientY,
            at: pointer.timeStamp,
          }
        : null;
  };

  const onPointerUp = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    if (!press || pointer.pointerId !== press.pointerId) return;
    const travel = Math.hypot(pointer.clientX - press.x, pointer.clientY - press.y);
    const duration = pointer.timeStamp - press.at;
    press = null;
    if (travel > TAP_MAX_TRAVEL_PX && duration <= FLICK_MAX_DURATION_MS) {
      flickedAt = pointer.timeStamp;
    }
  };

  // The browser took the gesture (a scroll, usually) and sends no click.
  const onPointerCancel = (event: Event) => {
    const pointer = event as unknown as PointerLike;
    if (press && pointer.pointerId === press.pointerId) press = null;
  };

  const onClick = (event: Event) => {
    if (flickedAt === null) return;
    const click = event as unknown as ClickLike;
    // A click made by script (`element.click()`) is not the touch's own.
    if (!click.isTrusted) return;
    const elapsed = click.timeStamp - flickedAt;
    flickedAt = null;
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
