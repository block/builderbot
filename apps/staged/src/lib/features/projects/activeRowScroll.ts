import type { ActionReturn } from 'svelte/action';

/** The sidebar state `scrollIfActive` reads and records. */
export interface ActiveRowTracking {
  /** The active project's row, which the scroll restore brings into view. */
  activeRow(): HTMLElement | null;
  setActiveRow(node: HTMLElement | null): void;
  /**
   * Whether the sidebar's scroll restore has run. Until it has, the restore
   * owns the scroll position and brings the active row into view itself.
   */
  scrollRestored(): boolean;
}

/**
 * The project-row action: records the active project's row and, once the
 * scroll restore has run, scrolls a row into view as it becomes active.
 *
 * That includes a row that mounts already active. The sidebar appends the
 * open project when its filter rules it out, so opening such a project (the
 * one a delete opens in the removed one's place, or one reached with ⌘↓/⌘↑)
 * adds its row at the end of the list, possibly below the fold.
 */
export function createScrollIfActive(tracking: ActiveRowTracking) {
  return function scrollIfActive(node: HTMLElement, active: boolean): ActionReturn<boolean> {
    function activate() {
      tracking.setActiveRow(node);
      if (tracking.scrollRestored()) {
        node.scrollIntoView({ block: 'nearest' });
      }
    }

    let currentActive = active;
    if (active) activate();
    return {
      update(nextActive: boolean) {
        if (currentActive === nextActive) return;
        currentActive = nextActive;
        if (nextActive) {
          activate();
        } else if (tracking.activeRow() === node) {
          tracking.setActiveRow(null);
        }
      },
      destroy() {
        if (tracking.activeRow() === node) {
          tracking.setActiveRow(null);
        }
      },
    };
  };
}
