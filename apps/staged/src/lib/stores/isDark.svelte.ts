/**
 * Reactive dark-mode signal derived from the theme system's
 * --theme-is-dark CSS custom property.
 *
 * A single MutationObserver watches the document root's style attribute
 * so that all consumers share one watcher.
 */

class DarkModeState {
  value = $state(false);
  private initialized = false;
  private observer: MutationObserver | null = null;

  /** Start watching. Subsequent calls are no-ops. */
  init(): void {
    if (this.initialized) return;
    this.initialized = true;
    this.update();
    this.observer = new MutationObserver(() => this.update());
    this.observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['style'],
    });
  }

  private update(): void {
    this.value =
      getComputedStyle(document.documentElement).getPropertyValue('--theme-is-dark').trim() === '1';
  }
}

export const darkMode = new DarkModeState();
