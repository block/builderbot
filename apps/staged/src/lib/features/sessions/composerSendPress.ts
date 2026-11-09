// composerSendPress.ts — what a press on the composer's Send button does to
// focus, and when the send itself runs. Pure state machines, so ChatComposer
// only wires DOM events to them.

/** Pointer types that raise the on-screen keyboard's text input. */
function isTouchPointer(pointerType: string): boolean {
  return pointerType === 'touch' || pointerType === 'pen';
}

export interface TouchPressFocusGuard {
  /** `pointerdown` on the button. Records the press; never cancels it. */
  pointerDown(pointerType: string): void;
  /**
   * `mousedown` on the button: whether to cancel it, which keeps focus (and
   * the on-screen keyboard) in the message box. Only the compatibility
   * mousedown of a touch or pen press is cancelled, and not while the box
   * holds an IME or dictation composition: moving focus is what commits it.
   * On Android, Gboard keeps the word being typed open as a composition, so
   * there a Send usually lets focus go and the keyboard drop, by design.
   */
  mouseDown(composing: boolean): boolean;
  /** `click` or `pointercancel`: the press is over. */
  reset(): void;
}

/**
 * Keep focus in the message box when Send is tapped on a touch screen.
 *
 * Only the compatibility `mousedown` is cancelled, never the `pointerdown`:
 * cancelling a pointerdown tells the browser to suppress the compatibility
 * mouse events, and Chromium then still moves focus. The press's pointer
 * type is all `pointerdown` contributes, and it is dropped as soon as the
 * press ends (its click, or a pointercancel when the touch became a scroll),
 * so a press whose mousedown never came cannot leak into a later mouse press
 * or a keyboard / VoiceOver activation. A press that ended with neither
 * (none is known to) cancels at most the next mousedown. The click itself is
 * never blocked.
 */
export function createTouchPressFocusGuard(): TouchPressFocusGuard {
  let touchPress = false;
  return {
    pointerDown(pointerType) {
      touchPress = isTouchPointer(pointerType);
    },
    mouseDown(composing) {
      const cancel = touchPress && !composing;
      touchPress = false;
      return cancel;
    },
    reset() {
      touchPress = false;
    },
  };
}

export interface CompositionSendGate {
  /** Send was pressed: true to send now, false when it waits (see below). */
  press(composing: boolean): boolean;
  /** The composition ended: true when a press is waiting and should send now. */
  compositionEnded(): boolean;
  /** The composition was dropped without ending (the box's content replaced). */
  cancel(): void;
}

/**
 * Hold a Send pressed while the message box holds an IME or dictation
 * composition until that composition ends. Sending clears the box; a
 * composition still open over it would then commit its text into the
 * cleared box (or the draft be sent without it and the rest sent again).
 * Several presses during one composition send once.
 */
export function createCompositionSendGate(): CompositionSendGate {
  let waiting = false;
  return {
    press(composing) {
      if (!composing) {
        waiting = false;
        return true;
      }
      waiting = true;
      return false;
    },
    compositionEnded() {
      const send = waiting;
      waiting = false;
      return send;
    },
    cancel() {
      waiting = false;
    },
  };
}
