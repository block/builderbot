import { afterEach, describe, expect, it, vi } from 'vitest';
import { onRehydrate, signalRehydrate } from './rehydrateSignal';

describe('rehydrateSignal', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('calls every listener until it unsubscribes', () => {
    const first = vi.fn();
    const second = vi.fn();
    const stopFirst = onRehydrate(first);
    const stopSecond = onRehydrate(second);

    signalRehydrate();
    stopFirst();
    signalRehydrate();
    stopSecond();
    signalRehydrate();

    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(2);
  });

  it('keeps going when a listener throws', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const after = vi.fn();
    const stopFailing = onRehydrate(() => {
      throw new Error('broken');
    });
    const stopAfter = onRehydrate(after);

    expect(() => signalRehydrate()).not.toThrow();
    expect(after).toHaveBeenCalledTimes(1);

    stopFailing();
    stopAfter();
  });
});
