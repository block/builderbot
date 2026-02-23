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
  }

  function handleClick(event: MouseEvent) {
    if (
      event.target === event.currentTarget &&
      pointerDownOnBackdrop &&
      (options.canDismiss?.() ?? true)
    ) {
      options.onDismiss();
    }

    pointerDownOnBackdrop = false;
  }

  return { handlePointerDown, handleClick };
}
