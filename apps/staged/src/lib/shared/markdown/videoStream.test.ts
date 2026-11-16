// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The desktop build, whose video comes from the app's loopback listener
// (src-tauri note_media/stream.rs) rather than the `staged-media` protocol.
vi.mock('../../transport', () => ({ isTauri: true }));
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string, protocol: string) => `${protocol}://localhost/${path}`),
}));

import { renderMarkdown } from './renderMarkdown';

const id = '3f2a9c1e-1234-4567-8123-123456789abc';
const stream = `http://127.0.0.1:49152/${'5e'.repeat(32)}`;
const page = globalThis as { __STAGED_MEDIA_STREAM__?: string };

/** A note as the viewer mounts it; returns its first video. */
function mount(markdown: string): HTMLVideoElement {
  document.body.innerHTML = renderMarkdown(markdown);
  return document.querySelector('video')!;
}

describe('desktop note video', () => {
  beforeEach(() => {
    page.__STAGED_MEDIA_STREAM__ = stream;
  });

  afterEach(() => {
    delete page.__STAGED_MEDIA_STREAM__;
    document.body.innerHTML = '';
  });

  it.each(['mp4', 'webm', 'mov'])('renders %s on the stream, preview fragment and all', (ext) => {
    const html = renderMarkdown(`![Clip](staged-media://${id}.${ext})`);
    expect(html).toContain(
      `<video src="${stream}/${id}.${ext}#t=0.001" controls preload="metadata" playsinline`
    );
  });

  it('keeps images on the protocol', () => {
    const html = renderMarkdown(`![Shot](staged-media://${id}.png)`);
    expect(html).toContain(`<img src="staged-media://localhost/${id}.png"`);
    expect(html).not.toContain('127.0.0.1');
  });

  it('renders on the protocol when the app announced no stream', () => {
    delete page.__STAGED_MEDIA_STREAM__;
    const html = renderMarkdown(`![Clip](staged-media://${id}.mp4)`);
    expect(html).toContain(`<video src="staged-media://localhost/${id}.mp4#t=0.001" controls`);
  });

  // The page is told the address before the listener is known to answer.
  it('moves a video whose stream fails to load onto the protocol, once', () => {
    const video = mount(`![Clip](staged-media://${id}.mp4)`);
    expect(video.getAttribute('src')).toBe(`${stream}/${id}.mp4#t=0.001`);

    video.dispatchEvent(new Event('error'));
    expect(video.getAttribute('src')).toBe(`staged-media://localhost/${id}.mp4#t=0.001`);

    // A clip that fails there too (a codec WebKit lacks) stays failed.
    video.dispatchEvent(new Event('error'));
    expect(video.getAttribute('src')).toBe(`staged-media://localhost/${id}.mp4#t=0.001`);
  });

  it('moves only the video that failed', () => {
    const other = '3f2a9c1e-1234-4567-8123-123456789abd';
    mount(`![A](staged-media://${id}.mp4)\n\n![B](staged-media://${other}.webm)`);
    const [first, second] = [...document.querySelectorAll('video')];
    second.dispatchEvent(new Event('error'));
    expect(first.getAttribute('src')).toBe(`${stream}/${id}.mp4#t=0.001`);
    expect(second.getAttribute('src')).toBe(`staged-media://localhost/${other}.webm#t=0.001`);
  });

  it('leaves videos that are not note media alone', () => {
    mount(`![Clip](staged-media://${id}.mp4)`);
    const src = `${stream}/${id}.mp4`;
    document.body.innerHTML = `<video src="${src}"></video>`;
    const video = document.querySelector('video')!;
    video.dispatchEvent(new Event('error'));
    expect(video.getAttribute('src')).toBe(src);
  });
});
