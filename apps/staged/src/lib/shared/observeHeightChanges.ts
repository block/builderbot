// observeHeightChanges.ts — react to a box changing height, but not to it
// being detached or reattached.

/**
 * Call `onChange` when `el`'s content-box height changes from one non-zero
 * value to another. The first observation is not reported, nor are changes to
 * or from 0: a box detached from the document (or hidden) observes as 0 high,
 * and its owner restores it on reattach. Returns the cleanup; a no-op where
 * there is no ResizeObserver.
 */
export function observeHeightChanges(el: Element, onChange: () => void): () => void {
  if (typeof ResizeObserver === 'undefined') return () => {};
  let lastHeight = -1;
  const observer = new ResizeObserver((entries) => {
    const entry = entries[entries.length - 1];
    if (!entry) return;
    const height = entry.contentRect.height;
    const previous = lastHeight;
    lastHeight = height;
    if (previous > 0 && height > 0 && height !== previous) onChange();
  });
  observer.observe(el);
  return () => observer.disconnect();
}
