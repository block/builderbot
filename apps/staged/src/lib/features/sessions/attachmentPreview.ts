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

const PDF_DATA_URL_PREFIX = `data:${PDF_MIME_TYPE};base64,`;

/** Whether a `get_image_data` reply carries a PDF rather than an image. */
export function isPdfDataUrl(dataUrl: string): boolean {
  return dataUrl.startsWith(PDF_DATA_URL_PREFIX);
}

/** The bytes of a base64 data URL. */
export function dataUrlBytes(dataUrl: string): Uint8Array<ArrayBuffer> {
  const binary = atob(dataUrl.slice(dataUrl.indexOf(',') + 1));
  const bytes = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/**
 * Object URL for showing a PDF in the webview's native viewer. An `<iframe>`
 * will not load a `data:` URL, so the bytes go through a Blob; the caller
 * revokes the URL when the viewer closes.
 */
export function pdfBlobUrl(dataUrl: string): string {
  return URL.createObjectURL(new Blob([dataUrlBytes(dataUrl)], { type: PDF_MIME_TYPE }));
}
