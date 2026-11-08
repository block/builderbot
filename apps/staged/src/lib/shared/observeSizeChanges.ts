// observeSizeChanges.ts — react to a box being resized, but not to it being
// detached or reattached.

/**
 * Call `onChange` when `el`'s content box changes width or height while it is
 * laid out: from one size with both dimensions non-zero to another. The first
 * observation is not reported, nor are changes to or from a size with a 0
 * dimension: a box detached from the document (or hidden) observes as 0×0,
 * and its owner restores it on reattach. Returns the cleanup; a no-op where
 * there is no ResizeObserver.
 */
export function observeSizeChanges(el: Element, onChange: () => void): () => void {
  if (typeof ResizeObserver === 'undefined') return () => {};
  let lastWidth = 0;
  let lastHeight = 0;
  const observer = new ResizeObserver((entries) => {
    const entry = entries[entries.length - 1];
    if (!entry) return;
    const { width, height } = entry.contentRect;
    const wasLaidOut = lastWidth > 0 && lastHeight > 0;
    const changed = width !== lastWidth || height !== lastHeight;
    lastWidth = width;
    lastHeight = height;
    if (wasLaidOut && width > 0 && height > 0 && changed) onChange();
  });
  observer.observe(el);
  return () => observer.disconnect();
}
