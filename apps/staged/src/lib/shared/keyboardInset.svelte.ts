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
let initialSyncFrame = 0;
// Last inset written to `:root`, or null when nothing has been written. Any
// write to the root's `style` attribute fires the MutationObserver in
// isDark.svelte.ts, so an unconditional write on every viewport resize would
// trigger a whole-document style recalc for a value that never changed.
let writtenInset: number | null = null;

function syncKeyboardInset() {
  const vv = trackedViewport;
  if (!vv) return;
  // The keyboard occupies the gap between the layout viewport's bottom and the
  // visual viewport's bottom (its height plus however far it has scrolled down).
  const inset = Math.max(0, window.innerHeight - vv.height - vv.offsetTop);
  keyboard.inset = inset;
  if (inset === writtenInset) return;
  writtenInset = inset;
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
  // The initial read forces a style/layout flush. Subscribers call this from
  // onMount — e.g. a dialog that has just been portalled in — so reading
  // synchronously would make the mount flush pay that recalc before the
  // first frame can paint. Defer it to the next frame instead.
  if (!initialSyncFrame) {
    initialSyncFrame = requestAnimationFrame(() => {
      initialSyncFrame = 0;
      syncKeyboardInset();
    });
  }

  return () => {
    subscriberCount = Math.max(0, subscriberCount - 1);
    if (subscriberCount === 0) {
      cancelAnimationFrame(initialSyncFrame);
      initialSyncFrame = 0;
      trackedViewport?.removeEventListener('resize', syncKeyboardInset);
      trackedViewport?.removeEventListener('scroll', syncKeyboardInset);
      trackedViewport = null;
      // Drop the inset so a stale keyboard height can't linger on `:root`.
      // Consumers fall back via `var(--keyboard-inset, 0px)`, so only write
      // when a non-zero value is actually there to clear.
      keyboard.inset = 0;
      if (writtenInset !== null && writtenInset !== 0) {
        document.documentElement.style.setProperty('--keyboard-inset', '0px');
      }
      writtenInset = null;
    }
  };
}
