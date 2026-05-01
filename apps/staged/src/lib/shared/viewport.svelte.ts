const MOBILE_BREAKPOINT_PX = 768;

export const viewport = $state({
  isMobile: false,
});

let mediaQuery: MediaQueryList | null = null;
let subscriberCount = 0;

function syncViewport() {
  viewport.isMobile = mediaQuery?.matches ?? false;
}

export function watchViewport(): () => void {
  if (typeof window === 'undefined') return () => {};

  subscriberCount += 1;

  if (!mediaQuery) {
    mediaQuery = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT_PX}px)`);
    mediaQuery.addEventListener('change', syncViewport);
  }
  syncViewport();

  return () => {
    subscriberCount = Math.max(0, subscriberCount - 1);
    if (subscriberCount === 0 && mediaQuery) {
      mediaQuery.removeEventListener('change', syncViewport);
      mediaQuery = null;
    }
  };
}
