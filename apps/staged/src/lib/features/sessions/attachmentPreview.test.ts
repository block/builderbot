import { describe, expect, it } from 'vitest';
import type { Image } from '../../types';
import { isPdfMimeType, localPreview } from './attachmentPreview';

function image(filename: string, mimeType: string): Image {
  return {
    id: 'img-1',
    branchId: null,
    projectId: 'p',
    sessionId: 'pending',
    filename,
    mimeType,
    sizeBytes: 3,
    createdAt: 0,
  };
}

describe('localPreview', () => {
  it('uses the uploaded bytes when the browser label matches the stored image MIME', () => {
    expect(localPreview(image('shot.png', 'image/png'), 'image/png', 'AAAA')).toEqual({
      dataUrl: 'data:image/png;base64,AAAA',
      mimeType: 'image/png',
      filename: 'shot.png',
    });
  });

  it('defers to the backend thumbnail for relabelled files and PDFs', () => {
    // A WebP saved as .png is stored as image/webp; the label disagrees.
    expect(localPreview(image('shot.webp', 'image/webp'), 'image/png', 'AAAA')).toBeNull();
    // PDF bytes are not something an <img> can show.
    expect(
      localPreview(image('doc.pdf', 'application/pdf'), 'application/pdf', 'JVBERi0=')
    ).toBeNull();
  });
});

describe('isPdfMimeType', () => {
  it('recognises only the PDF MIME', () => {
    expect(isPdfMimeType('application/pdf')).toBe(true);
    expect(isPdfMimeType('image/png')).toBe(false);
  });
});
