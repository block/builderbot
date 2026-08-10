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

/**
 * Alt+Tab (and any other focus-stealing chord) lands its keyup in another
 * window, so without this the flag would stay set for as long as any tracker
 * is mounted — and with cards on most surfaces, that means app-wide action
 * buttons stuck showing the stop icon.
 */
function handleBlur() {
  held = false;
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
    window.addEventListener('blur', handleBlur);
  }
  return () => {
    if (--trackers === 0) {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
      window.removeEventListener('blur', handleBlur);
      held = false;
    }
  };
}
