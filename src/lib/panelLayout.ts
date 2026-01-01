/**
 * Panel Layout Utility
 *
 * Calculates CSS classes for the two-pane diff layout based on
 * collapse, hover, and zoom state.
 *
 * The layout uses flex ratios:
 * - Default: 40/60 (before gets 40%, after gets 60%)
 * - Focused (hovered): 60/40 (hovered pane expands)
 * - Zoomed (space held): 90/10 (zoomed pane dominates)
 * - Collapsed: 10/90 (collapsed pane minimized)
 *
 * Rebuildable: This module owns layout logic. To change the layout
 * behavior or ratios, only this file needs modification.
 */

export interface PanelState {
  /** Whether the before pane is collapsed (for new files) */
  beforeCollapsed: boolean;
  /** Whether the after pane is collapsed (for deleted files) */
  afterCollapsed: boolean;
  /** Whether the before pane is hovered */
  beforeHovered: boolean;
  /** Whether the after pane is hovered */
  afterHovered: boolean;
  /** Whether space key is held (zoom modifier) */
  spaceHeld: boolean;
}

export interface PanelClasses {
  before: string;
  after: string;
}

/**
 * Calculate CSS classes for both panes based on current state.
 */
export function getPanelClasses(state: PanelState): PanelClasses {
  const beforeClasses: string[] = ['diff-pane', 'before-pane'];
  const afterClasses: string[] = ['diff-pane', 'after-pane'];

  // Collapsed state
  if (state.beforeCollapsed) {
    beforeClasses.push('collapsed');
  }
  if (state.afterCollapsed) {
    afterClasses.push('collapsed');
  }

  // Focused state (hovered but not collapsed)
  if (state.beforeHovered && !state.beforeCollapsed) {
    beforeClasses.push('focused');
    if (state.spaceHeld) {
      beforeClasses.push('zoomed');
    }
  }
  if (state.afterHovered && !state.afterCollapsed) {
    afterClasses.push('focused');
    if (state.spaceHeld) {
      afterClasses.push('zoomed');
    }
  }

  return {
    before: beforeClasses.join(' '),
    after: afterClasses.join(' '),
  };
}

/**
 * Create initial panel state from file characteristics.
 */
export function createInitialPanelState(isNewFile: boolean, isDeletedFile: boolean): PanelState {
  return {
    beforeCollapsed: isNewFile,
    afterCollapsed: isDeletedFile,
    beforeHovered: false,
    afterHovered: false,
    spaceHeld: false,
  };
}

/**
 * Set up space key handling for zoom modifier.
 * Returns a cleanup function.
 */
export function setupSpaceKeyHandler(onSpaceChange: (held: boolean) => void): () => void {
  function handleKeyDown(e: KeyboardEvent) {
    if (e.code === 'Space' && !e.repeat) {
      const target = e.target as HTMLElement;
      // Allow space in text inputs
      if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') {
        return;
      }
      // Blur focusable elements to capture space
      if (document.activeElement instanceof HTMLElement) {
        document.activeElement.blur();
      }
      e.preventDefault();
      e.stopPropagation();
      onSpaceChange(true);
    }
  }

  function handleKeyUp(e: KeyboardEvent) {
    if (e.code === 'Space') {
      onSpaceChange(false);
    }
  }

  window.addEventListener('keydown', handleKeyDown, { capture: true });
  window.addEventListener('keyup', handleKeyUp, { capture: true });

  return () => {
    window.removeEventListener('keydown', handleKeyDown, { capture: true });
    window.removeEventListener('keyup', handleKeyUp, { capture: true });
  };
}
