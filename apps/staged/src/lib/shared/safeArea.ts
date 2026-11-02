export interface EdgeOffsets {
  top: string;
  right: string;
  bottom: string;
  left: string;
}

/**
 * `distance` from each screen edge, measured from the safe area rather than
 * the screen: clear of the notch, status bar and home indicator of a
 * viewport-fit=cover page (index.html). Each inset is 0 where there is
 * nothing to avoid, which leaves `distance` unchanged.
 */
export function safeAreaOffset(distance: string): EdgeOffsets {
  const edge = (side: keyof EdgeOffsets) => `calc(${distance} + env(safe-area-inset-${side}))`;
  return { top: edge('top'), right: edge('right'), bottom: edge('bottom'), left: edge('left') };
}
