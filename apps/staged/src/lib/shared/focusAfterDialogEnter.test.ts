import { describe, expect, it, vi } from 'vitest';
import { focusAfterDialogEnter } from './focusAfterDialogEnter';

// vitest runs in node: elements are stubbed down to what the helper touches
// (`closest`, `getAnimations`, `isConnected`, `focus`).

interface FakeAnimation {
  playState: AnimationPlayState;
  finished: Promise<unknown>;
}

function deferredAnimation(): FakeAnimation & { finish(): void; cancel(): void } {
  let finish!: () => void;
  let cancel!: () => void;
  const finished = new Promise<unknown>((resolve, reject) => {
    finish = () => resolve(undefined);
    cancel = () => reject(new Error('AbortError'));
  });
  return { playState: 'running', finished, finish, cancel };
}

function fakeElement(dialog: { getAnimations(): FakeAnimation[] } | null) {
  return {
    isConnected: true,
    focus: vi.fn(),
    closest: vi.fn(() => dialog),
  } as unknown as HTMLElement & { focus: ReturnType<typeof vi.fn>; isConnected: boolean };
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

describe('focusAfterDialogEnter', () => {
  it('focuses at once outside a full-screen dialog', () => {
    const el = fakeElement(null);
    focusAfterDialogEnter(el);
    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('focuses at once when the full-screen dialog has finished entering', () => {
    const el = fakeElement({
      getAnimations: () => [{ playState: 'finished', finished: Promise.resolve() }],
    });
    focusAfterDialogEnter(el);
    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('waits for the enter animation of a full-screen dialog', async () => {
    const enter = deferredAnimation();
    const el = fakeElement({ getAnimations: () => [enter] });

    focusAfterDialogEnter(el);
    await settle();
    expect(el.focus).not.toHaveBeenCalled();

    enter.finish();
    await settle();
    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('still focuses when the animation is cancelled', async () => {
    const enter = deferredAnimation();
    const el = fakeElement({ getAnimations: () => [enter] });

    focusAfterDialogEnter(el);
    enter.cancel();
    await settle();

    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('skips the focus if the dialog closed before it finished entering', async () => {
    const enter = deferredAnimation();
    const el = fakeElement({ getAnimations: () => [enter] });

    focusAfterDialogEnter(el);
    el.isConnected = false;
    enter.finish();
    await settle();

    expect(el.focus).not.toHaveBeenCalled();
  });

  it('runs a custom focus in place of el.focus()', async () => {
    const enter = deferredAnimation();
    const el = fakeElement({ getAnimations: () => [enter] });
    const focus = vi.fn();

    focusAfterDialogEnter(el, focus);
    enter.finish();
    await settle();

    expect(focus).toHaveBeenCalledWith(el);
    expect(el.focus).not.toHaveBeenCalled();
  });

  it('ignores a missing element', () => {
    expect(() => focusAfterDialogEnter(null)).not.toThrow();
  });
});
