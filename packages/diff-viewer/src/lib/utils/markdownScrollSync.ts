/**
 * Markdown Scroll Sync
 *
 * Synchronizes scroll position between two rendered markdown panes using
 * proportional mapping. When one pane scrolls to X% of its content, the
 * other pane scrolls to X% of its content.
 *
 * This is different from code-mode scroll sync (which uses line-level
 * alignment mapping) because rendered markdown has unpredictable heights —
 * a heading might be huge on one side and absent on the other.
 */

/**
 * Set up proportional scroll sync between two scrollable containers.
 * Returns a cleanup function that removes all event listeners.
 *
 * @param containerA - First scrollable container
 * @param containerB - Second scrollable container
 */
export function setupMarkdownScrollSync(
  containerA: HTMLElement,
  containerB: HTMLElement
): () => void {
  // Guard against feedback loops: when we programmatically set scrollTop
  // on one container, it fires a scroll event — we must ignore it.
  let syncing = false;

  function getScrollFraction(el: HTMLElement): number {
    const maxScroll = el.scrollHeight - el.clientHeight;
    if (maxScroll <= 0) return 0;
    return el.scrollTop / maxScroll;
  }

  function setScrollFraction(el: HTMLElement, fraction: number): void {
    const maxScroll = el.scrollHeight - el.clientHeight;
    if (maxScroll <= 0) return;
    el.scrollTop = fraction * maxScroll;
  }

  function handleScrollA() {
    if (syncing) return;
    syncing = true;
    setScrollFraction(containerB, getScrollFraction(containerA));
    // Use rAF to release the guard after the browser has
    // processed the programmatic scroll and fired its event.
    requestAnimationFrame(() => {
      syncing = false;
    });
  }

  function handleScrollB() {
    if (syncing) return;
    syncing = true;
    setScrollFraction(containerA, getScrollFraction(containerB));
    requestAnimationFrame(() => {
      syncing = false;
    });
  }

  containerA.addEventListener('scroll', handleScrollA, { passive: true });
  containerB.addEventListener('scroll', handleScrollB, { passive: true });

  return () => {
    containerA.removeEventListener('scroll', handleScrollA);
    containerB.removeEventListener('scroll', handleScrollB);
  };
}
