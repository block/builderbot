import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Image } from '../../types';
import {
  dataUrlBytes,
  isPdfDataUrl,
  isPdfMimeType,
  localPreview,
  pdfBlobUrl,
} from './attachmentPreview';

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

describe('PDF data URLs', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('recognises only the PDF MIME', () => {
    expect(isPdfMimeType('application/pdf')).toBe(true);
    expect(isPdfMimeType('image/png')).toBe(false);
    expect(isPdfDataUrl('data:application/pdf;base64,JVBERi0=')).toBe(true);
    expect(isPdfDataUrl('data:image/png;base64,iVBOR')).toBe(false);
  });

  it('decodes the bytes and hands them to the viewer as a PDF blob URL', () => {
    expect(Array.from(dataUrlBytes('data:application/pdf;base64,JVBERi0='))).toEqual([
      0x25, 0x50, 0x44, 0x46, 0x2d,
    ]);
    const createObjectURL = vi.fn((blob: Blob) => `blob:${blob.type}:${blob.size}`);
    vi.stubGlobal('URL', { ...URL, createObjectURL });
    expect(pdfBlobUrl('data:application/pdf;base64,JVBERi0=')).toBe('blob:application/pdf:5');
    expect(createObjectURL).toHaveBeenCalledTimes(1);
  });
});
