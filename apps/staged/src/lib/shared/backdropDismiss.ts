/**
 * Shared handlers for dismissing a modal only when pointer-down and click both
 * occur on the backdrop itself.
 */
interface BackdropDismissOptions {
  onDismiss: () => void;
  canDismiss?: () => boolean;
}

export function createBackdropDismissHandlers(options: BackdropDismissOptions) {
  let pointerDownOnBackdrop = false;

  function handlePointerDown(event: PointerEvent) {
    pointerDownOnBackdrop = event.target === event.currentTarget;
    if (pointerDownOnBackdrop) {
      event.stopPropagation();
    }
  }

  function handleClick(event: MouseEvent) {
    const clickedBackdrop = event.target === event.currentTarget;

    if (clickedBackdrop) {
      event.stopPropagation();
    }

    if (clickedBackdrop && pointerDownOnBackdrop && (options.canDismiss?.() ?? true)) {
      options.onDismiss();
    }

    pointerDownOnBackdrop = false;
  }

  return { handlePointerDown, handleClick };
}
