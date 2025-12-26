/**
 * Diff Keyboard Navigation
 *
 * Keyboard shortcuts for scrolling the diff viewer.
 */

export interface KeyboardNavConfig {
  scrollAmount: number; // pixels per keypress
  getScrollTarget: () => HTMLElement | null;
}

const DEFAULT_CONFIG: KeyboardNavConfig = {
  scrollAmount: 60, // ~3 lines
  getScrollTarget: () => null,
};

/**
 * Set up keyboard navigation handlers.
 * Returns a cleanup function.
 */
export function setupKeyboardNav(config: Partial<KeyboardNavConfig> = {}): () => void {
  const cfg = { ...DEFAULT_CONFIG, ...config };

  function handleKeydown(e: KeyboardEvent) {
    const target = cfg.getScrollTarget();
    if (!target) return;

    if (e.ctrlKey && (e.key === 'p' || e.key === 'P')) {
      e.preventDefault();
      target.scrollTop -= cfg.scrollAmount;
    } else if (e.ctrlKey && (e.key === 'n' || e.key === 'N')) {
      e.preventDefault();
      target.scrollTop += cfg.scrollAmount;
    } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      const eventTarget = e.target as HTMLElement;
      if (eventTarget.tagName === 'INPUT' || eventTarget.tagName === 'TEXTAREA') return;

      e.preventDefault();
      target.scrollTop += e.key === 'ArrowUp' ? -cfg.scrollAmount : cfg.scrollAmount;
    }
  }

  window.addEventListener('keydown', handleKeydown);
  return () => window.removeEventListener('keydown', handleKeydown);
}
