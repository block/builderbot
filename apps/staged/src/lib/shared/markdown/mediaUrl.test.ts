import { afterEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../transport', () => ({ isTauri: true }));
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string, protocol: string) => `${protocol}://localhost/${path}`),
}));
import { convertFileSrc } from '@tauri-apps/api/core';
import {
  collectStagedMediaImageIds,
  parseStagedMediaReference,
  protocolUrlForStreamedVideo,
  resolvePdfAttachmentUrl,
  resolveStagedMediaUrl,
  resolveStagedVideoUrl,
} from './mediaUrl';
it('delegates platform-specific protocol URLs to Tauri', () => {
  expect(resolveStagedMediaUrl('id', 'png')).toBe('staged-media://localhost/id.png');
  expect(convertFileSrc).toHaveBeenCalledWith('id.png', 'staged-media');
});

// The desktop app announces its loopback video listener in a page global
// (src-tauri note_media/stream.rs).
describe('desktop video', () => {
  const id = '0f3b9d2a-1c4e-4a5b-8d6e-7f8091a2b3c4';
  const stream = `http://127.0.0.1:49152/${'5e'.repeat(32)}`;
  const announce = (value: unknown) =>
    ((globalThis as { __STAGED_MEDIA_STREAM__?: unknown }).__STAGED_MEDIA_STREAM__ = value);

  afterEach(() => {
    delete (globalThis as { __STAGED_MEDIA_STREAM__?: unknown }).__STAGED_MEDIA_STREAM__;
  });

  it('is streamed from the listener the app announced, and nothing else is', () => {
    announce(stream);
    for (const ext of ['mp4', 'webm', 'mov']) {
      expect(resolveStagedVideoUrl(id, ext)).toBe(`${stream}/${id}.${ext}`);
    }
    expect(resolveStagedMediaUrl(id, 'png')).toBe(`staged-media://localhost/${id}.png`);
    expect(resolveStagedMediaUrl(id, 'mp4')).toBe(`staged-media://localhost/${id}.mp4`);
    expect(resolvePdfAttachmentUrl(id)).toBe(`staged-media://localhost/${id}.pdf`);
  });

  it('stays on the protocol when no listener was announced', () => {
    expect(resolveStagedVideoUrl(id, 'mp4')).toBe(`staged-media://localhost/${id}.mp4`);
  });

  it.each([
    ['another host', `http://example.com:49152/${'5e'.repeat(32)}`],
    ['a host name', `http://localhost:49152/${'5e'.repeat(32)}`],
    ['another scheme', `https://127.0.0.1:49152/${'5e'.repeat(32)}`],
    ['no port', `http://127.0.0.1/${'5e'.repeat(32)}`],
    ['a short secret', 'http://127.0.0.1:49152/5e5e'],
    ['no secret', 'http://127.0.0.1:49152'],
    ['a longer path', `${stream}/extra`],
    ['a trailing slash', `${stream}/`],
    ['userinfo', `http://127.0.0.1:49152@example.com/${'5e'.repeat(32)}`],
    ['a non-string', { toString: () => stream }],
  ])('only takes a loopback address with a secret: not %s', (_, value) => {
    announce(value);
    expect(resolveStagedVideoUrl(id, 'mp4')).toBe(`staged-media://localhost/${id}.mp4`);
  });

  it('maps a streamed video back to the protocol, fragment and all', () => {
    expect(protocolUrlForStreamedVideo(`${stream}/${id}.mp4#t=0.001`)).toBe(
      `staged-media://localhost/${id}.mp4#t=0.001`
    );
    expect(protocolUrlForStreamedVideo(`${stream}/${id}.webm`)).toBe(
      `staged-media://localhost/${id}.webm`
    );
    // Needs no announcement: the page may be looking at a dead listener.
    for (const other of [
      `staged-media://localhost/${id}.mp4#t=0.001`,
      `/api/media/${id}.mp4`,
      `${stream}/${id}.png`,
      `${stream}/${id}.mp4?x=1`,
      `http://example.com/${'5e'.repeat(32)}/${id}.mp4`,
      '',
    ]) {
      expect(protocolUrlForStreamedVideo(other)).toBeNull();
    }
  });
});

it('resolves PDF attachments by id without admitting them as note embeds', () => {
  expect(resolvePdfAttachmentUrl('id')).toBe('staged-media://localhost/id.pdf');
  expect(convertFileSrc).toHaveBeenCalledWith('id.pdf', 'staged-media');
  expect(
    parseStagedMediaReference('staged-media://0f3b9d2a-1c4e-4a5b-8d6e-7f8091a2b3c4.pdf')
  ).toBeNull();
});

it('collects each embedded image id once, skipping videos and malformed references', () => {
  const a = '0f3b9d2a-1c4e-4a5b-8d6e-7f8091a2b3c4';
  const b = '1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d';
  const c = '2b3c4d5e-6f7a-4b8c-9d0e-1f2a3b4c5d6e';
  expect(
    collectStagedMediaImageIds(
      `![a](staged-media://${a}.png) ![b](staged-media://${b}.jpeg "t")\n` +
        `![again](staged-media://${a}.png) ![clip](staged-media://${c}.mp4)\n` +
        `![bad](staged-media://nope.png) ![svg](staged-media://${c}.svg)`
    )
  ).toEqual([a, b]);
  expect(collectStagedMediaImageIds('no media here')).toEqual([]);
});
