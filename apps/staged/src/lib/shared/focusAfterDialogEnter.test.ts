import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { autofocusOnOpen, focusAfterDialogEnter } from './focusAfterDialogEnter';

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
    ownerDocument: { activeElement: null as unknown },
  } as unknown as HTMLElement & {
    focus: ReturnType<typeof vi.fn>;
    isConnected: boolean;
    ownerDocument: { activeElement: unknown };
  };
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

const IPHONE = {
  userAgent:
    'Mozilla/5.0 (iPhone; CPU iPhone OS 27_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.2 Mobile/15E148 Safari/604.1',
  platform: 'iPhone',
  maxTouchPoints: 5,
};

beforeEach(() => {
  coarsePointer = true;
  vi.stubGlobal('window', {
    matchMedia: (query: string) => ({ matches: query === '(pointer: coarse)' && coarsePointer }),
  });
  vi.stubGlobal('navigator', IPHONE);
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

describe('autofocusOnOpen', () => {
  it('focuses with a mouse and keyboard', () => {
    coarsePointer = false;
    const el = fakeElement(fakeDialog(deferredAnimation()));
    const focus = vi.fn();

    autofocusOnOpen(el, focus);

    expect(focus).toHaveBeenCalledWith(el);
  });

  it('leaves an unfocused input alone on an iPhone, where it would get no keyboard', async () => {
    const el = fakeElement(null);

    autofocusOnOpen(el);
    await settle();

    expect(el.focus).not.toHaveBeenCalled();
  });

  it('leaves it alone on an iPad, which presents itself as a Mac', async () => {
    vi.stubGlobal('navigator', {
      userAgent:
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.2 Safari/605.1.15',
      platform: 'MacIntel',
      maxTouchPoints: 5,
    });
    const el = fakeElement(null);

    autofocusOnOpen(el);
    await settle();

    expect(el.focus).not.toHaveBeenCalled();
  });

  it("focuses on an Android phone, whose keyboard follows the opening tap's focus", () => {
    vi.stubGlobal('navigator', {
      userAgent:
        'Mozilla/5.0 (Linux; Android 16; Pixel 10) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Mobile Safari/537.36',
      platform: 'Linux armv81',
      maxTouchPoints: 5,
    });
    const el = fakeElement(null);
    const focus = vi.fn();

    autofocusOnOpen(el, focus);

    expect(focus).toHaveBeenCalledWith(el);
  });

  it('focuses on a Mac', () => {
    coarsePointer = false;
    vi.stubGlobal('navigator', {
      userAgent:
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)',
      platform: 'MacIntel',
      maxTouchPoints: 0,
    });
    const el = fakeElement(fakeDialog(deferredAnimation()));
    const focus = vi.fn();

    autofocusOnOpen(el, focus);

    expect(focus).toHaveBeenCalledWith(el);
  });

  it('still refocuses an input that already has focus on an iPhone', () => {
    // A mode switch that swaps the prompt under the caret puts it back.
    const el = fakeElement(null);
    el.ownerDocument.activeElement = el;
    const focus = vi.fn();

    autofocusOnOpen(el, focus);

    expect(focus).toHaveBeenCalledWith(el);
  });

  it('ignores a missing element', () => {
    expect(() => autofocusOnOpen(undefined)).not.toThrow();
  });
});
