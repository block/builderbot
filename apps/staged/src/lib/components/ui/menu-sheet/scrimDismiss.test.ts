import { describe, expect, it, vi } from 'vitest';
import { createScrimDismiss } from './scrimDismiss';

describe('createScrimDismiss', () => {
  it('ignores the trailing click of the tap that opened the sheet', () => {
    const dismiss = vi.fn();
    const scrim = createScrimDismiss(dismiss);

    // The opening tap went down on the trigger; only its click reaches the scrim.
    scrim.onclick();

    expect(dismiss).not.toHaveBeenCalled();
  });

  it('dismisses on the click of a tap that went down on the scrim', () => {
    const dismiss = vi.fn();
    const scrim = createScrimDismiss(dismiss);

    scrim.onpointerdown();
    scrim.onclick();

    expect(dismiss).toHaveBeenCalledTimes(1);
  });

  it('needs a fresh pointerdown for each dismissal', () => {
    const dismiss = vi.fn();
    const scrim = createScrimDismiss(dismiss);

    scrim.onpointerdown();
    scrim.onclick();
    scrim.onclick();
    expect(dismiss).toHaveBeenCalledTimes(1);

    scrim.onpointerdown();
    scrim.onclick();
    expect(dismiss).toHaveBeenCalledTimes(2);
  });

  it('a cancelled gesture disarms the scrim', () => {
    const dismiss = vi.fn();
    const scrim = createScrimDismiss(dismiss);

    scrim.onpointerdown();
    scrim.onpointercancel();
    scrim.onclick();

    expect(dismiss).not.toHaveBeenCalled();
  });
});
