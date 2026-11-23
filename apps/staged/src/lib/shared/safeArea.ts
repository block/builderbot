export interface EdgeOffsets {
  top: string;
  right: string;
  bottom: string;
  left: string;
}

/**
 * The bottom safe-area inset: `--safe-bottom`, defined once on `:root` in
 * app.css as the larger of the env() bottom inset and the inset
 * viewportFit.ts leaves behind when it drops `viewport-fit=cover`. Styles use
 * the property, never the env() inset itself, which reads 0 without cover
 * (safeAreaBottom.test.ts fails on one that does).
 */
export const SAFE_AREA_BOTTOM = 'var(--safe-bottom)';

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
