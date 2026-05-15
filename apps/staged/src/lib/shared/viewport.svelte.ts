const MOBILE_BREAKPOINT_PX = 768;

export const viewport = $state({
  isMobile: false,
  hasKeyboard: true,
});

let mediaQuery: MediaQueryList | null = null;
let coarsePointerQuery: MediaQueryList | null = null;
let noHoverQuery: MediaQueryList | null = null;
let subscriberCount = 0;

function syncViewport() {
  viewport.isMobile = mediaQuery?.matches ?? false;
  viewport.hasKeyboard = !(
    (coarsePointerQuery?.matches ?? false) ||
    (noHoverQuery?.matches ?? false)
  );
}

export function watchViewport(): () => void {
  if (typeof window === 'undefined') return () => {};

  subscriberCount += 1;

  if (!mediaQuery) {
    mediaQuery = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT_PX}px)`);
    mediaQuery.addEventListener('change', syncViewport);
  }
  if (!coarsePointerQuery) {
    coarsePointerQuery = window.matchMedia('(pointer: coarse)');
    coarsePointerQuery.addEventListener('change', syncViewport);
  }
  if (!noHoverQuery) {
    noHoverQuery = window.matchMedia('(hover: none)');
    noHoverQuery.addEventListener('change', syncViewport);
  }
  syncViewport();

  return () => {
    subscriberCount = Math.max(0, subscriberCount - 1);
    if (subscriberCount === 0) {
      mediaQuery?.removeEventListener('change', syncViewport);
      coarsePointerQuery?.removeEventListener('change', syncViewport);
      noHoverQuery?.removeEventListener('change', syncViewport);
      mediaQuery = null;
      coarsePointerQuery = null;
      noHoverQuery = null;
    }
  };
}
