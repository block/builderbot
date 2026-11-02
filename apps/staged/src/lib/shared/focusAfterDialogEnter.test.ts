import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { focusAfterDialogEnter } from './focusAfterDialogEnter';

// vitest runs in node: elements are stubbed down to what the helper touches
// (`closest`, `getAnimations`, `dataset.state`, `isConnected`, `focus`), and
// `window` down to the `(pointer: coarse)` query.

interface FakeAnimation {
  playState: AnimationPlayState;
  finished: Promise<unknown>;
}

interface FakeDialog {
  dataset: { state: string };
  getAnimations(): FakeAnimation[];
}

let coarsePointer: boolean;

function deferredAnimation(): FakeAnimation & { finish(): void; cancel(): void } {
  let finish!: () => void;
  let cancel!: () => void;
  const finished = new Promise<unknown>((resolve, reject) => {
    finish = () => resolve(undefined);
    cancel = () => reject(new Error('AbortError'));
  });
  return { playState: 'running', finished, finish, cancel };
}

function fakeDialog(...animations: FakeAnimation[]): FakeDialog {
  return { dataset: { state: 'open' }, getAnimations: () => animations };
}

function fakeElement(dialog: FakeDialog | null) {
  return {
    isConnected: true,
    focus: vi.fn(),
    closest: vi.fn(() => dialog),
  } as unknown as HTMLElement & { focus: ReturnType<typeof vi.fn>; isConnected: boolean };
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

beforeEach(() => {
  coarsePointer = true;
  vi.stubGlobal('window', {
    matchMedia: (query: string) => ({ matches: query === '(pointer: coarse)' && coarsePointer }),
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('focusAfterDialogEnter', () => {
  it('focuses at once outside a full-screen dialog', () => {
    const el = fakeElement(null);
    focusAfterDialogEnter(el);
    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('focuses at once when the full-screen dialog has finished entering', () => {
    const el = fakeElement(fakeDialog({ playState: 'finished', finished: Promise.resolve() }));
    focusAfterDialogEnter(el);
    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('focuses at once with a mouse, in a narrow window where the dialog is full-screen too', () => {
    coarsePointer = false;
    const el = fakeElement(fakeDialog(deferredAnimation()));

    focusAfterDialogEnter(el);

    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('waits for the enter animation of a full-screen dialog', async () => {
    const enter = deferredAnimation();
    const el = fakeElement(fakeDialog(enter));

    focusAfterDialogEnter(el);
    await settle();
    expect(el.focus).not.toHaveBeenCalled();

    enter.finish();
    await settle();
    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('still focuses when the animation is cancelled', async () => {
    const enter = deferredAnimation();
    const el = fakeElement(fakeDialog(enter));

    focusAfterDialogEnter(el);
    enter.cancel();
    await settle();

    expect(el.focus).toHaveBeenCalledTimes(1);
  });

  it('skips the focus if the dialog started closing before it finished entering', async () => {
    // Closing swaps the enter animation for the exit one, which cancels it
    // while the dialog is still in the document.
    const enter = deferredAnimation();
    const dialog = fakeDialog(enter);
    const el = fakeElement(dialog);

    focusAfterDialogEnter(el);
    dialog.dataset.state = 'closed';
    enter.cancel();
    await settle();

    expect(el.focus).not.toHaveBeenCalled();
  });

  it('skips the focus if the dialog was removed before it finished entering', async () => {
    const enter = deferredAnimation();
    const el = fakeElement(fakeDialog(enter));

    focusAfterDialogEnter(el);
    el.isConnected = false;
    enter.finish();
    await settle();

    expect(el.focus).not.toHaveBeenCalled();
  });

  it('runs a custom focus in place of el.focus()', async () => {
    const enter = deferredAnimation();
    const el = fakeElement(fakeDialog(enter));
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
