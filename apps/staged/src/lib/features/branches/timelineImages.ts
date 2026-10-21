import type { BranchTimeline, Image } from '../../types';
import { attachErrorReason, type AttachRejection } from '../sessions/attachErrors';

/** One image headed straight onto a branch timeline (dropped or picked). */
export interface TimelineImageUpload {
  name: string;
  store: () => Promise<Pick<Image, 'id'>>;
}

export interface StoreImageHandlers {
  /** The image at `index` was stored as `image`. */
  onStored?: (index: number, image: Pick<Image, 'id'>) => void;
  /** The image at `index` was rejected; its reason is in the returned list. */
  onRejected?: (index: number) => void;
}

/**
 * Stores a batch of images one at a time, in the order given. A web store
 * holds the whole file in memory several times over (bytes, base64, request
 * body), so going one by one keeps a large Photo Library pick in mobile
 * Safari to one file's worth. It also keeps the batch in pick order: an
 * image's timeline position is when the store inserted it.
 *
 * A rejection does not stop the batch. Each is reported through `onRejected`
 * as it happens and returned with the rest, so the caller can report the
 * whole batch in one message.
 */
export async function storeImagesInOrder(
  images: TimelineImageUpload[],
  { onStored, onRejected }: StoreImageHandlers = {}
): Promise<AttachRejection[]> {
  const rejections: AttachRejection[] = [];
  for (const [index, { name, store }] of images.entries()) {
    let image: Pick<Image, 'id'>;
    try {
      image = await store();
    } catch (e) {
      console.error('Failed to add image:', e);
      rejections.push({ name, reason: attachErrorReason(e) });
      onRejected?.(index);
      continue;
    }
    onStored?.(index, image);
  }
  return rejections;
}

/**
 * Returns a `storeImagesInOrder` that runs each batch only after every earlier
 * one has finished. A pick or drop made while a batch is still storing then
 * waits its turn, so across batches too only one file is stored at a time,
 * and batches land on the timeline in the order they were made instead of
 * interleaving.
 */
export function createImageStoreQueue() {
  let tail: Promise<unknown> = Promise.resolve();
  return (images: TimelineImageUpload[], handlers?: StoreImageHandlers) => {
    const batch = tail.then(() => storeImagesInOrder(images, handlers));
    tail = batch.catch(() => {});
    return batch;
  };
}

/**
 * Drops the "adding..." rows whose stored image is already in `timeline`.
 * Every stored image publishes a branch change, and the reload it triggers
 * brings in the image's real row; pruning in that same update swaps the row
 * for the image instead of showing both. Returns `adds` itself when nothing
 * has landed.
 */
export function pruneLandedImages<T extends { imageId?: string }>(
  adds: T[],
  timeline: Pick<BranchTimeline, 'images'>
): T[] {
  if (!adds.some((add) => add.imageId)) return adds;
  const landed = new Set(timeline.images.map((image) => image.id));
  const next = adds.filter((add) => !add.imageId || !landed.has(add.imageId));
  return next.length === adds.length ? adds : next;
}
