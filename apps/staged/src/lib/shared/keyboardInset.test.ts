import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The module's `$state` rune compiles away in the app build; under vitest it
// is a plain global call, so stub it as identity (preferences.test.ts
// precedent). vitest runs in node here, so `window`/`document` are stubbed
// just far enough for the module to drive them.

class FakeVisualViewport extends EventTarget {
  height = 800;
  offsetTop = 0;
}

let viewport: FakeVisualViewport;
let setProperty: ReturnType<typeof vi.fn>;
let rafCallbacks: Array<() => void>;

function flushFrames(): void {
  const pending = rafCallbacks;
  rafCallbacks = [];
  for (const cb of pending) cb();
}

async function importModule() {
  return await import('./keyboardInset.svelte');
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  viewport = new FakeVisualViewport();
  setProperty = vi.fn();
  rafCallbacks = [];
  vi.stubGlobal('window', { innerHeight: 800, visualViewport: viewport });
  vi.stubGlobal('document', { documentElement: { style: { setProperty } } });
  vi.stubGlobal('requestAnimationFrame', (cb: () => void) => {
    rafCallbacks.push(cb);
    return rafCallbacks.length;
  });
  vi.stubGlobal('cancelAnimationFrame', () => {});
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('watchKeyboardInset', () => {
  it('writes the inset on first sync and skips writes while it is unchanged', async () => {
    const { keyboard, watchKeyboardInset } = await importModule();
    watchKeyboardInset();
    flushFrames();

    expect(keyboard.inset).toBe(0);
    expect(setProperty).toHaveBeenCalledTimes(1);
    expect(setProperty).toHaveBeenCalledWith('--keyboard-inset', '0px');

    viewport.dispatchEvent(new Event('resize'));
    viewport.dispatchEvent(new Event('scroll'));
    expect(setProperty).toHaveBeenCalledTimes(1);
  });

  it('writes again when the inset changes', async () => {
    const { keyboard, watchKeyboardInset } = await importModule();
    watchKeyboardInset();
    flushFrames();
    setProperty.mockClear();

    viewport.height = 500;
    viewport.dispatchEvent(new Event('resize'));
    expect(keyboard.inset).toBe(300);
    expect(setProperty).toHaveBeenCalledTimes(1);
    expect(setProperty).toHaveBeenCalledWith('--keyboard-inset', '300px');

    viewport.dispatchEvent(new Event('resize'));
    expect(setProperty).toHaveBeenCalledTimes(1);
  });

  it('cleanup does not write 0px when the inset never left zero', async () => {
    const { keyboard, watchKeyboardInset } = await importModule();
    const unwatch = watchKeyboardInset();
    flushFrames();
    setProperty.mockClear();

    unwatch();
    expect(keyboard.inset).toBe(0);
    expect(setProperty).not.toHaveBeenCalled();
  });

  it('cleanup writes 0px after a non-zero inset was written', async () => {
    const { keyboard, watchKeyboardInset } = await importModule();
    const unwatch = watchKeyboardInset();
    flushFrames();
    viewport.height = 500;
    viewport.dispatchEvent(new Event('resize'));
    setProperty.mockClear();

    unwatch();
    expect(keyboard.inset).toBe(0);
    expect(setProperty).toHaveBeenCalledTimes(1);
    expect(setProperty).toHaveBeenCalledWith('--keyboard-inset', '0px');
  });

  it('cleanup does not write when the initial sync never ran', async () => {
    const { watchKeyboardInset } = await importModule();
    const unwatch = watchKeyboardInset();
    unwatch();
    expect(setProperty).not.toHaveBeenCalled();
  });
});
