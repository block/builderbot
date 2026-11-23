export interface EdgeOffsets {
  top: string;
  right: string;
  bottom: string;
  left: string;
}

/**
 * The bottom safe-area inset. Where viewportFit.ts has dropped
 * `viewport-fit=cover`, every `env()` inset reads 0 while the page still runs
 * under the home indicator, so the guard leaves the inset it last read in
 * `--safe-bottom-fallback`. Stylesheets spell this expression out.
 */
export const SAFE_AREA_BOTTOM =
  'max(env(safe-area-inset-bottom), var(--safe-bottom-fallback, 0px))';

/**
 * `distance` from each screen edge, measured from the safe area rather than
 * the screen: clear of the notch, status bar and home indicator of a
 * viewport-fit=cover page (index.html). Each inset is 0 where there is
 * nothing to avoid, which leaves `distance` unchanged.
 */
export function safeAreaOffset(distance: string): EdgeOffsets {
  const edge = (side: 'top' | 'right' | 'left') =>
    `calc(${distance} + env(safe-area-inset-${side}))`;
  return {
    top: edge('top'),
    right: edge('right'),
    bottom: `calc(${distance} + ${SAFE_AREA_BOTTOM})`,
    left: edge('left'),
  };
}
