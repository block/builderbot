import { attachErrorReason, type AttachRejection } from '../sessions/attachErrors';

/** One image headed straight onto a branch timeline (dropped or picked). */
export interface TimelineImageUpload {
  name: string;
  store: () => Promise<unknown>;
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
  onRejected?: (index: number) => void
): Promise<AttachRejection[]> {
  const rejections: AttachRejection[] = [];
  for (const [index, { name, store }] of images.entries()) {
    try {
      await store();
    } catch (e) {
      console.error('Failed to add image:', e);
      rejections.push({ name, reason: attachErrorReason(e) });
      onRejected?.(index);
    }
  }
  return rejections;
}
