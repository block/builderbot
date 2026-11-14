/**
 * Whether a drag that fired a dragleave is still over `container`: what it
 * moved on to (an element, when the event names one) is inside. Not every
 * engine fills in a dragleave's `relatedTarget`, so without one the pointer
 * decides.
 *
 * A dragleave bubbles from every element the drag crosses, so a container
 * that draws a drop indicator for its children uses this to tell the drag
 * leaving it from the drag moving about inside it.
 */
export function dragStillInside(
  container: Pick<Element, 'contains' | 'getBoundingClientRect'>,
  event: Pick<DragEvent, 'relatedTarget' | 'clientX' | 'clientY'>
): boolean {
  const next = event.relatedTarget as Node | null;
  if (next) return container.contains(next);
  const box = container.getBoundingClientRect();
  return (
    event.clientX >= box.left &&
    event.clientX < box.right &&
    event.clientY >= box.top &&
    event.clientY < box.bottom
  );
}
