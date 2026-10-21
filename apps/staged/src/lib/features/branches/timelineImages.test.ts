import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ImageTimelineItem } from '../../types';
import { formatAttachErrors } from '../sessions/attachErrors';
import { createImageStoreQueue, pruneLandedImages, storeImagesInOrder } from './timelineImages';

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => (resolve = r));
  return { promise, resolve };
}

/** A store that waits for `gate` (if any), logs `id` to `order`, and returns it. */
function storeAs(id: string, order: string[], gate?: Promise<void>) {
  return vi.fn(async () => {
    order.push(`start ${id}`);
    await gate;
    order.push(`end ${id}`);
    return { id };
  });
}

describe('storeImagesInOrder', () => {
  beforeEach(() => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('starts each store only after the previous one settles', async () => {
    const order: string[] = [];
    const first = deferred();
    const second = deferred();
    const storeFirst = storeAs('a', order, first.promise);
    const storeSecond = storeAs('b', order, second.promise);

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

  it('keeps going past a rejection, reporting each image by index', async () => {
    const order: string[] = [];
    const onStored = vi.fn();
    const onRejected = vi.fn();
    const store = (name: string, error?: unknown) => async () => {
      order.push(name);
      if (error !== undefined) throw error;
      return { id: `id-${name}` };
    };

    const rejections = await storeImagesInOrder(
      [
        { name: 'big.png', store: store('big.png', new Error('media exceeds 10 MB limit')) },
        { name: 'ok.png', store: store('ok.png') },
        { name: 'bad.gif', store: store('bad.gif', 'unsupported image format') },
      ],
      { onStored, onRejected }
    );

    expect(order).toEqual(['big.png', 'ok.png', 'bad.gif']);
    expect(rejections).toEqual([
      { name: 'big.png', reason: 'media exceeds 10 MB limit' },
      { name: 'bad.gif', reason: 'unsupported image format' },
    ]);
    expect(onStored.mock.calls).toEqual([[1, { id: 'id-ok.png' }]]);
    expect(onRejected.mock.calls).toEqual([[0], [2]]);
  });

  it('reports a batch of rejections as one message', async () => {
    const tooBig = async (): Promise<{ id: string }> => {
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

describe('createImageStoreQueue', () => {
  beforeEach(() => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('holds a second batch until the first has stored every image', async () => {
    const order: string[] = [];
    const a1 = deferred();
    const a2 = deferred();
    const storeImages = createImageStoreQueue();

    const firstBatch = storeImages([
      { name: 'a1.jpg', store: storeAs('a1', order, a1.promise) },
      { name: 'a2.jpg', store: storeAs('a2', order, a2.promise) },
    ]);
    const storeB1 = storeAs('b1', order);
    const secondBatch = storeImages([
      { name: 'b1.jpg', store: storeB1 },
      { name: 'b2.jpg', store: storeAs('b2', order) },
    ]);

    a1.resolve();
    await vi.waitFor(() => expect(order).toContain('start a2'));
    expect(storeB1).not.toHaveBeenCalled();

    a2.resolve();
    await Promise.all([firstBatch, secondBatch]);
    expect(order).toEqual([
      'start a1',
      'end a1',
      'start a2',
      'end a2',
      'start b1',
      'end b1',
      'start b2',
      'end b2',
    ]);
  });

  it('returns each batch its own rejections, and a rejection does not stall the queue', async () => {
    const storeImages = createImageStoreQueue();
    const rejected = async (): Promise<{ id: string }> => {
      throw new Error('could not open PDF');
    };

    const [first, second] = await Promise.all([
      storeImages([{ name: 'scan.pdf', store: rejected }]),
      storeImages([{ name: 'ok.png', store: async () => ({ id: 'ok' }) }]),
    ]);

    expect(first).toEqual([{ name: 'scan.pdf', reason: 'could not open PDF' }]);
    expect(second).toEqual([]);
  });

  it('keeps running batches after a handler throws', async () => {
    const storeImages = createImageStoreQueue();
    const store = async () => ({ id: 'x' });

    const first = storeImages([{ name: 'a.png', store }], {
      onStored: () => {
        throw new Error('handler bug');
      },
    });
    const second = storeImages([{ name: 'b.png', store }]);

    await expect(first).rejects.toThrow('handler bug');
    expect(await second).toEqual([]);
  });
});

describe('pruneLandedImages', () => {
  function timelineWith(...ids: string[]) {
    return {
      images: ids.map((id): ImageTimelineItem => ({
        id,
        filename: `${id}.png`,
        mimeType: 'image/png',
        sizeBytes: 1,
        sessionId: null,
        sessionStatus: null,
        completionReason: null,
        createdAt: 0,
      })),
    };
  }

  it('drops rows whose stored image is on the timeline', () => {
    const adds = [
      { key: 'note', imageId: undefined },
      { key: 'landed', imageId: 'img-1' },
      { key: 'stored', imageId: 'img-2' },
      { key: 'storing' },
    ];

    expect(pruneLandedImages(adds, timelineWith('img-1', 'other')).map((a) => a.key)).toEqual([
      'note',
      'stored',
      'storing',
    ]);
  });

  it('returns the same rows when nothing has landed', () => {
    const adds = [{ key: 'stored', imageId: 'img-2' }, { key: 'storing' }];

    const unstored = [{ key: 'storing', imageId: undefined }];

    expect(pruneLandedImages(adds, timelineWith('img-1'))).toBe(adds);
    expect(pruneLandedImages(unstored, timelineWith())).toBe(unstored);
  });
});
