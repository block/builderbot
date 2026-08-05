/**
 * Shared Alt-key tracking for the quick stop-action affordance: while Alt is
 * held, running-action buttons swap to a stop icon. One pair of window
 * listeners is shared by every mounted tracker via reference counting.
 */

let held = $state(false);
let trackers = 0;

function handleKeyDown(e: KeyboardEvent) {
  if (e.key === 'Alt') held = true;
}

function handleKeyUp(e: KeyboardEvent) {
  if (e.key === 'Alt') held = false;
}

export const altKey = {
  get held() {
    return held;
  },
};

/** Track the Alt key while mounted; returns a cleanup (usable as an onMount return). */
export function trackAltKey(): () => void {
  if (trackers++ === 0) {
    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
  }
  return () => {
    if (--trackers === 0) {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
      held = false;
    }
  };
}
