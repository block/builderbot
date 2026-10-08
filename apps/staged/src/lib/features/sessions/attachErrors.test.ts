import { describe, expect, it } from 'vitest';
import { attachBasename, attachErrorReason, formatAttachErrors } from './attachErrors';

describe('formatAttachErrors', () => {
  it('returns null when nothing was rejected', () => {
    expect(formatAttachErrors([])).toBeNull();
  });

  it('names a single rejected file with its reason', () => {
    expect(formatAttachErrors([{ name: 'bad.webp', reason: 'Unrecognized image signature' }])).toBe(
      'Could not attach bad.webp: Unrecognized image signature'
    );
  });

  it('lists every rejected name when they share a reason', () => {
    expect(
      formatAttachErrors([
        { name: 'bad.webp', reason: 'Unrecognized image signature' },
        { name: 'worse.png', reason: 'Unrecognized image signature' },
      ])
    ).toBe('Could not attach 2 files (bad.webp, worse.png): Unrecognized image signature');
  });

  it('pairs each name with its reason when reasons differ', () => {
    expect(
      formatAttachErrors([
        { name: 'bad.webp', reason: 'Unrecognized image signature' },
        { name: 'huge.png', reason: 'Image exceeds 10 MiB' },
      ])
    ).toBe(
      'Could not attach 2 files: bad.webp (Unrecognized image signature); huge.png (Image exceeds 10 MiB)'
    );
  });
});

describe('attachErrorReason', () => {
  it('uses the Error message and stringifies anything else', () => {
    expect(attachErrorReason(new Error('too big'))).toBe('too big');
    expect(attachErrorReason('plain')).toBe('plain');
  });
});

describe('attachBasename', () => {
  it('strips POSIX and Windows directories', () => {
    expect(attachBasename('/tmp/shots/one.png')).toBe('one.png');
    expect(attachBasename('C:\\shots\\two.png')).toBe('two.png');
    expect(attachBasename('three.png')).toBe('three.png');
  });
});
