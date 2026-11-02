/**
 * Tap-to-dismiss for a scrim that only honours a gesture that began on it.
 *
 * bits-ui opens a menu on the trigger's touch `pointerup`, before the tap's
 * synthesized `click` is dispatched, so a sheet mounts under the still-lifting
 * finger and its scrim covers the trigger. Whether WebKit then delivers that
 * trailing click to the element now under the finger is up to WebKit (some
 * quick taps do, slower ones never do), and a scrim that closed on any click
 * would close the sheet it was just opened with. Arming on the scrim's own
 * `pointerdown` ties dismissal to the gesture instead of to timing: the
 * opening tap went down on the trigger, so its click finds the scrim unarmed
 * and is ignored, while a tap that starts on the scrim arms it and closes on
 * its own click.
 *
 * The rule bits-ui's outside-interaction dismissal already follows (a layer
 * only owns a pointerdown it saw while mounted), applied to the click.
 */
export interface ScrimDismiss {
  onpointerdown(): void;
  onpointercancel(): void;
  onclick(): void;
}

export function createScrimDismiss(dismiss: () => void): ScrimDismiss {
  let armed = false;
  return {
    onpointerdown() {
      armed = true;
    },
    // The browser took the gesture over (a scroll, a system gesture): no click
    // will follow for it.
    onpointercancel() {
      armed = false;
    },
    onclick() {
      if (!armed) return;
      armed = false;
      dismiss();
    },
  };
}
