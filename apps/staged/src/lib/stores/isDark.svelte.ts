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
    const root = document.documentElement;
    // applyChromeTheme writes the variable inline on <html>, so the inline
    // value is authoritative whenever present. Reading it is free, whereas
    // getComputedStyle forces a whole-document style recalc (~70-90 ms on a
    // large tree) every time anything touches the root's style attribute.
    // Fall back to the computed value only when no inline theme is set, e.g.
    // a theme applied purely through a stylesheet.
    const inline = root.style.getPropertyValue('--theme-is-dark').trim();
    const value = inline || getComputedStyle(root).getPropertyValue('--theme-is-dark').trim();
    this.value = value === '1';
  }
}

export const darkMode = new DarkModeState();
