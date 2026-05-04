import { describe, expect, it, vi } from 'vitest';
import { createBackdropDismissHandlers } from './backdropDismiss';

function createEvent(target: object, currentTarget: object) {
  return {
    target,
    currentTarget,
    stopPropagation: vi.fn(),
  };
}

describe('createBackdropDismissHandlers', () => {
  it('dismisses and consumes clicks that start and end on the backdrop', () => {
    const backdrop = {};
    const onDismiss = vi.fn();
    const handlers = createBackdropDismissHandlers({ onDismiss });
    const pointerDown = createEvent(backdrop, backdrop);
    const click = createEvent(backdrop, backdrop);

    handlers.handlePointerDown(pointerDown as unknown as PointerEvent);
    handlers.handleClick(click as unknown as MouseEvent);

    expect(onDismiss).toHaveBeenCalledOnce();
    expect(pointerDown.stopPropagation).toHaveBeenCalledOnce();
    expect(click.stopPropagation).toHaveBeenCalledOnce();
  });

  it('consumes backdrop clicks even when the pointer down started inside the dialog', () => {
    const backdrop = {};
    const dialog = {};
    const onDismiss = vi.fn();
    const handlers = createBackdropDismissHandlers({ onDismiss });
    const pointerDown = createEvent(dialog, backdrop);
    const click = createEvent(backdrop, backdrop);

    handlers.handlePointerDown(pointerDown as unknown as PointerEvent);
    handlers.handleClick(click as unknown as MouseEvent);

    expect(onDismiss).not.toHaveBeenCalled();
    expect(pointerDown.stopPropagation).not.toHaveBeenCalled();
    expect(click.stopPropagation).toHaveBeenCalledOnce();
  });

  it('does not consume clicks inside the dialog', () => {
    const backdrop = {};
    const dialog = {};
    const onDismiss = vi.fn();
    const handlers = createBackdropDismissHandlers({ onDismiss });
    const pointerDown = createEvent(dialog, backdrop);
    const click = createEvent(dialog, backdrop);

    handlers.handlePointerDown(pointerDown as unknown as PointerEvent);
    handlers.handleClick(click as unknown as MouseEvent);

    expect(onDismiss).not.toHaveBeenCalled();
    expect(pointerDown.stopPropagation).not.toHaveBeenCalled();
    expect(click.stopPropagation).not.toHaveBeenCalled();
  });
});
