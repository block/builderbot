/**
 * Which timeline footer buttons collapse into the `…` menu at a given width.
 *
 * The buttons' label tiers (full label → `+` and short label → icon only) are
 * pure CSS container queries. This last tier can't be: the menu's content is
 * portaled out of the `timeline` container, so no `@container` query can decide
 * which items it should show. Both sides therefore read the same measured width
 * through this module, keeping button and menu in sync by construction.
 */

/** Left-aligned footer actions, in the order they appear on the card. */
export type FooterAction = 'note' | 'commit' | 'review';

/** Which of the three session actions the card is currently offering. */
export type FooterActionAvailability = Partial<Record<FooterAction, boolean>>;

export type FooterOverflowState = Record<FooterAction, boolean>;

/**
 * Order actions leave the footer in. Review goes first — it is the widest label
 * and the most situational — then Commit, leaving Note as the last one standing.
 */
const OVERFLOW_ORDER: readonly FooterAction[] = ['review', 'commit', 'note'];

/**
 * Minimum timeline width (px) that still fits N icon-only buttons alongside the
 * `…` trigger and the right-aligned PR/Diff group. Index is the button count, so
 * index 0 is the always-fits case.
 *
 * These sit below the 480px icon-only tier in `BranchTimeline.svelte`, which is
 * where the buttons have already shed their labels.
 */
const MIN_WIDTH_FOR_BUTTONS: readonly number[] = [0, 260, 320, 380];

/**
 * Decide which footer buttons to hide at `widthPx`, given which ones exist.
 *
 * Actions the card isn't offering are reported as hidden, so callers can render
 * the menu straight from this result without re-checking availability.
 */
export function computeFooterOverflow(
  widthPx: number,
  available: FooterActionAvailability
): FooterOverflowState {
  const hidden: FooterOverflowState = {
    note: !available.note,
    commit: !available.commit,
    review: !available.review,
  };

  // A zero width means the container hasn't been measured yet (first paint,
  // or an off-screen card). Showing everything matches the pre-overflow
  // behaviour and self-corrects on the first ResizeObserver callback.
  if (widthPx <= 0) return hidden;

  let visible = OVERFLOW_ORDER.filter((action) => !hidden[action]).length;
  for (const action of OVERFLOW_ORDER) {
    if (visible === 0 || widthPx >= MIN_WIDTH_FOR_BUTTONS[visible]) break;
    if (hidden[action]) continue;
    hidden[action] = true;
    visible -= 1;
  }
  return hidden;
}

/**
 * The actions that overflowed into the menu, in their original button order.
 * Excludes actions the card never offered.
 */
export function overflowedActions(
  overflow: FooterOverflowState,
  available: FooterActionAvailability
): FooterAction[] {
  return (['note', 'commit', 'review'] as const).filter(
    (action) => available[action] && overflow[action]
  );
}
