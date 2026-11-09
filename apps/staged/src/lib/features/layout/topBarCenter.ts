/**
 * The room for the top bar's centre slot (the project name once its heading
 * scrolls away). On desktop the slot is centred over the whole bar, so it can
 * only be as wide as twice the distance from the bar's centre to the nearer
 * of what's beside it: the left actions and title, or the right actions. A
 * narrow window would otherwise draw the name over a button.
 */
export interface TopBarExtents {
  /** The bar's left and right edges. */
  barLeft: number;
  barRight: number;
  /** Where the content on the left (back button, title) ends. */
  leftEnd: number;
  /** Where the right actions start (the bar's right edge without any). */
  rightStart: number;
  /** Space kept on each side of the slot. */
  gap: number;
}

/** The centre slot's widest width, in px, never negative. */
export function centerSlotRoom({
  barLeft,
  barRight,
  leftEnd,
  rightStart,
  gap,
}: TopBarExtents): number {
  const centre = (barLeft + barRight) / 2;
  const halfRoom = Math.min(centre - leftEnd, rightStart - centre) - gap;
  return Math.max(0, Math.floor(2 * halfRoom));
}
