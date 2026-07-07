// keyboardInset.svelte.ts — track the on-screen keyboard height.
//
// Exposes the height of the on-screen keyboard both as reactive state
// (`keyboard.inset`) and as a CSS custom property (`--keyboard-inset` on
// `:root`), so full-screen layouts can shrink to the space above the keyboard
// and pin their footer to its top edge. Mirrors the watch/unsubscribe pattern
// of viewport.svelte.ts.
//
// `window.visualViewport` is the source of truth (supported on iOS WebKit and
// Android). On older/desktop webviews it is undefined — the inset stays 0 and
// behaviour is unchanged.

export const keyboard = $state({ inset: 0 });

let trackedViewport: VisualViewport | null = null;
let subscriberCount = 0;

function syncKeyboardInset() {
  const vv = trackedViewport;
  if (!vv) return;
  // The keyboard occupies the gap between the layout viewport's bottom and the
  // visual viewport's bottom (its height plus however far it has scrolled down).
  const inset = Math.max(0, window.innerHeight - vv.height - vv.offsetTop);
  keyboard.inset = inset;
  document.documentElement.style.setProperty('--keyboard-inset', `${inset}px`);
}

export function watchKeyboardInset(): () => void {
  if (typeof window === 'undefined' || !window.visualViewport) return () => {};

  subscriberCount += 1;

  if (!trackedViewport) {
    trackedViewport = window.visualViewport;
    trackedViewport.addEventListener('resize', syncKeyboardInset);
    trackedViewport.addEventListener('scroll', syncKeyboardInset);
  }
  syncKeyboardInset();

  return () => {
    subscriberCount = Math.max(0, subscriberCount - 1);
    if (subscriberCount === 0) {
      trackedViewport?.removeEventListener('resize', syncKeyboardInset);
      trackedViewport?.removeEventListener('scroll', syncKeyboardInset);
      trackedViewport = null;
      // Drop the inset so a stale keyboard height can't linger on `:root`.
      keyboard.inset = 0;
      document.documentElement.style.setProperty('--keyboard-inset', '0px');
    }
  };
}
