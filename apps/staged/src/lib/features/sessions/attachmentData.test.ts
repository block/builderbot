import { describe, expect, it } from 'vitest';
import { fileToBase64 } from './attachmentData';

function decode(base64: string): number[] {
  return Array.from(atob(base64), (c) => c.charCodeAt(0));
}

describe('fileToBase64', () => {
  it('encodes an empty file as an empty string', async () => {
    expect(await fileToBase64(new Blob([]))).toBe('');
  });

  it('encodes binary bytes, not text', async () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff]);
    expect(await fileToBase64(new Blob([png]))).toBe('iVBORw0KGgoA/w==');
  });

  it('round-trips a file spanning several chunks', async () => {
    const bytes = new Uint8Array(8192 * 2 + 17).map((_, i) => (i * 31) % 256);
    const encoded = await fileToBase64(new File([bytes], 'photo.jpg', { type: 'image/jpeg' }));
    expect(decode(encoded)).toEqual(Array.from(bytes));
  });
});
