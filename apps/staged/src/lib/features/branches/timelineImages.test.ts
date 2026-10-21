import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { formatAttachErrors } from '../sessions/attachErrors';
import { storeImagesInOrder } from './timelineImages';

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => (resolve = r));
  return { promise, resolve };
}

describe('storeImagesInOrder', () => {
  beforeEach(() => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('starts each store only after the previous one settles', async () => {
    const first = deferred();
    const second = deferred();
    const storeFirst = vi.fn(() => first.promise);
    const storeSecond = vi.fn(() => second.promise);

    const done = storeImagesInOrder([
      { name: 'a.png', store: storeFirst },
      { name: 'b.png', store: storeSecond },
    ]);

    await vi.waitFor(() => expect(storeFirst).toHaveBeenCalled());
    expect(storeSecond).not.toHaveBeenCalled();

    first.resolve();
    await vi.waitFor(() => expect(storeSecond).toHaveBeenCalled());

    second.resolve();
    expect(await done).toEqual([]);
  });

  it('keeps going past a rejection and returns every rejection in order', async () => {
    const order: string[] = [];
    const onRejected = vi.fn();
    const store = (name: string, error?: unknown) => async () => {
      order.push(name);
      if (error !== undefined) throw error;
    };

    const rejections = await storeImagesInOrder(
      [
        { name: 'big.png', store: store('big.png', new Error('media exceeds 10 MB limit')) },
        { name: 'ok.png', store: store('ok.png') },
        { name: 'bad.gif', store: store('bad.gif', 'unsupported image format') },
      ],
      onRejected
    );

    expect(order).toEqual(['big.png', 'ok.png', 'bad.gif']);
    expect(rejections).toEqual([
      { name: 'big.png', reason: 'media exceeds 10 MB limit' },
      { name: 'bad.gif', reason: 'unsupported image format' },
    ]);
    expect(onRejected.mock.calls).toEqual([[0], [2]]);
  });

  it('reports a batch of rejections as one message', async () => {
    const tooBig = async () => {
      throw new Error('media exceeds 10 MB limit');
    };

    const rejections = await storeImagesInOrder([
      { name: 'one.jpg', store: tooBig },
      { name: 'two.jpg', store: tooBig },
    ]);

    expect(formatAttachErrors(rejections)).toBe(
      'Could not attach 2 files (one.jpg, two.jpg): media exceeds 10 MB limit'
    );
  });
});
