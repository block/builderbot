import type { AttachmentThumbnail, Image } from '../../types';

/**
 * What a thumbnail slot knows about a chat attachment: the backend's
 * `get_attachment_thumbnail` reply (an image's own data URL, or a PNG of a
 * PDF's first page — `dataUrl` is null when a PDF could not be rendered, and
 * the slot shows a document icon instead).
 */
export type AttachmentPreview = AttachmentThumbnail;

export const PDF_MIME_TYPE = 'application/pdf';

export function isPdfMimeType(mimeType: string): boolean {
  return mimeType === PDF_MIME_TYPE;
}

/**
 * Preview built from the bytes just uploaded, when they are what got stored
 * and an `<img>` can show them. The stored MIME follows the file signature,
 * which may differ from the browser's label, and a PDF's bytes are not an
 * image; in both cases the caller fetches the backend thumbnail instead.
 */
export function localPreview(
  image: Image,
  browserType: string,
  base64: string
): AttachmentPreview | null {
  if (image.mimeType !== browserType || !image.mimeType.startsWith('image/')) return null;
  return {
    dataUrl: `data:${image.mimeType};base64,${base64}`,
    mimeType: image.mimeType,
    filename: image.filename,
  };
}
