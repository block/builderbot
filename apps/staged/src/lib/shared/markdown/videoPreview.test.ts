// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../transport', () => ({ isTauri: false }));

import { renderMarkdown } from './renderMarkdown';
import { resolveStagedVideoSrc, withVideoPreviewFragment } from './videoPreview';

const id = '3f2a9c1e-1234-4567-8123-123456789abc';

describe('the preview fragment', () => {
  it('is added to the web and the desktop URL alike', () => {
    expect(resolveStagedVideoSrc(id, 'webm')).toBe(`/api/media/${id}.webm#t=0.001`);
    expect(withVideoPreviewFragment(`staged-media://localhost/${id}.mov`)).toBe(
      `staged-media://localhost/${id}.mov#t=0.001`
    );
    // Windows and Android serve custom protocols from an http origin.
    expect(withVideoPreviewFragment(`http://staged-media.localhost/${id}.mp4`)).toBe(
      `http://staged-media.localhost/${id}.mp4#t=0.001`
    );
  });

  // The loopback stream is the desktop app's (videoStream.test.ts); a browser
  // or phone could not reach it, whatever a page global says.
  it('rides on the web URL even where a stream address is set', () => {
    const page = globalThis as { __STAGED_MEDIA_STREAM__?: string };
    page.__STAGED_MEDIA_STREAM__ = `http://127.0.0.1:49152/${'5e'.repeat(32)}`;
    try {
      expect(resolveStagedVideoSrc(id, 'mp4')).toBe(`/api/media/${id}.mp4#t=0.001`);
    } finally {
      delete page.__STAGED_MEDIA_STREAM__;
    }
  });

  it('follows a query string', () => {
    expect(withVideoPreviewFragment('/api/media/a.mp4?token=x&v=2')).toBe(
      '/api/media/a.mp4?token=x&v=2#t=0.001'
    );
  });

  it('leaves a fragment that already names a time alone', () => {
    for (const url of ['/a.mp4#t=5', '/a.mp4#t=5,10', '/a.mp4?q=1#track=video&t=2']) {
      expect(withVideoPreviewFragment(url)).toBe(url);
    }
  });

  it('joins a fragment that names no time', () => {
    expect(withVideoPreviewFragment('/a.mp4#track=video')).toBe('/a.mp4#track=video&t=0.001');
    expect(withVideoPreviewFragment('/a.mp4#')).toBe('/a.mp4#t=0.001');
    // `t` only as a field name, not as the tail of another.
    expect(withVideoPreviewFragment('/a.mp4#xywh=0,0,10,10&start=3')).toBe(
      '/a.mp4#xywh=0,0,10,10&start=3&t=0.001'
    );
  });
});

/*
 * jsdom has no media pipeline, so the element's state is scripted: the test
 * plays WebKit (fires the events, presents a frame or not) and reads back the
 * seeks the page made. That a seek really paints is checked in a WKWebView.
 */
describe('the script seek for a preview the fragment did not paint', () => {
  interface Scripted {
    video: HTMLVideoElement;
    /** Times the page seeked to, in order. */
    seeks: number[];
    state: { paused: boolean; seeking: boolean; duration: number; time: number };
    /** WebKit: the fragment's seek, or the page's last one, has finished. */
    seeked(): void;
    /** WebKit: a frame reached the screen. */
    presentFrame(): void;
  }

  function script(video: HTMLVideoElement): Scripted {
    const seeks: number[] = [];
    const state = { paused: true, seeking: false, duration: 120, time: 0 };
    let onFrame: (() => void) | null = null;
    Object.defineProperties(video, {
      paused: { get: () => state.paused },
      seeking: { get: () => state.seeking },
      duration: { get: () => state.duration },
      currentSrc: { get: () => video.getAttribute('src') },
      currentTime: {
        get: () => state.time,
        set: (time: number) => {
          seeks.push(time);
          state.time = time;
        },
      },
      requestVideoFrameCallback: {
        value: (callback: () => void) => {
          onFrame = callback;
          return 1;
        },
      },
    });
    return {
      video,
      seeks,
      state,
      seeked: () => video.dispatchEvent(new Event('seeked')),
      presentFrame: () => onFrame?.(),
    };
  }

  /** A note video as the viewer mounts it, with its metadata just in. */
  function mountVideo(markdown = `![Clip](staged-media://${id}.mp4)`): Scripted {
    document.body.innerHTML = renderMarkdown(markdown);
    const scripted = script(document.querySelector('video')!);
    scripted.video.dispatchEvent(new Event('loadedmetadata'));
    return scripted;
  }

  /** WebKit applies the fragment: a seek of its own, not one from the page. */
  function fragmentSeek(scripted: Scripted) {
    scripted.state.time = 0.001;
    scripted.seeked();
  }

  const grace = () => vi.advanceTimersByTimeAsync(250);

  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = '';
  });

  it('does nothing when the fragment painted a frame', async () => {
    const clip = mountVideo();
    fragmentSeek(clip);
    clip.presentFrame();
    await grace();
    await grace();
    expect(clip.seeks).toEqual([]);
  });

  it('seeks on until a frame shows, then returns to the start', async () => {
    const clip = mountVideo();
    fragmentSeek(clip);
    // Not before WebKit has had time to present the frame.
    await vi.advanceTimersByTimeAsync(200);
    expect(clip.seeks).toEqual([]);
    await vi.advanceTimersByTimeAsync(50);
    expect(clip.seeks).toEqual([0.05]);

    // Still nothing (a track that starts later than that): the next time.
    clip.seeked();
    await grace();
    expect(clip.seeks).toEqual([0.05, 0.1]);

    clip.seeked();
    clip.presentFrame();
    await grace();
    // Back where the fragment put it, so play and the timeline start there.
    expect(clip.seeks).toEqual([0.05, 0.1, 0.001]);

    clip.seeked();
    await grace();
    await grace();
    expect(clip.seeks).toHaveLength(3);
  });

  it('waits for each seek to finish before judging it', async () => {
    const clip = mountVideo();
    fragmentSeek(clip);
    await grace();
    expect(clip.seeks).toEqual([0.05]);
    await grace();
    await grace();
    expect(clip.seeks).toEqual([0.05]);
  });

  it('gives up after the last time and returns to the start', async () => {
    const clip = mountVideo();
    fragmentSeek(clip);
    for (let i = 0; i < 8; i++) {
      await grace();
      clip.seeked();
    }
    expect(clip.seeks).toEqual([0.05, 0.1, 0.25, 0.5, 1, 0.001]);
  });

  it('stays inside a clip shorter than the next time', async () => {
    const clip = mountVideo();
    clip.state.duration = 0.08;
    fragmentSeek(clip);
    await grace();
    clip.seeked();
    await grace();
    expect(clip.seeks).toEqual([0.05, 0.001]);
  });

  it.each([
    ['pressed play', (clip: Scripted) => void (clip.state.paused = false)],
    ['moved the timeline', (clip: Scripted) => void (clip.state.time = 42)],
    ['is still scrubbing', (clip: Scripted) => void (clip.state.seeking = true)],
    ['closed the note', (clip: Scripted) => clip.video.remove()],
    ['got another clip in the element', (clip: Scripted) => clip.video.setAttribute('src', 'x')],
  ])('leaves the video alone once the reader %s', async (_, takeOver) => {
    const clip = mountVideo();
    fragmentSeek(clip);
    await grace();
    expect(clip.seeks).toEqual([0.05]);

    takeOver(clip);
    clip.seeked();
    clip.presentFrame();
    await grace();
    await grace();
    // No further time, and no return to the start either.
    expect(clip.seeks).toEqual([0.05]);
  });

  it('does not judge a page that is hidden, which presents no frames', async () => {
    const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
    const clip = mountVideo();
    fragmentSeek(clip);
    await grace();
    await grace();
    expect(clip.seeks).toEqual([]);

    hidden.mockReturnValue(false);
    document.dispatchEvent(new Event('visibilitychange'));
    clip.presentFrame();
    await grace();
    expect(clip.seeks).toEqual([]);
    hidden.mockRestore();
  });

  it('ignores videos that are not note media', async () => {
    resolveStagedVideoSrc(id, 'mp4');
    document.body.innerHTML = '<video src="/elsewhere.mp4#t=0.001"></video>';
    const clip = script(document.querySelector('video')!);
    clip.video.dispatchEvent(new Event('loadedmetadata'));
    fragmentSeek(clip);
    await grace();
    expect(clip.seeks).toEqual([]);
  });

  it('needs no frame callback support to leave the fragment working', async () => {
    document.body.innerHTML = renderMarkdown(`![Clip](staged-media://${id}.mp4)`);
    const video = document.querySelector('video')!;
    expect(video.getAttribute('src')).toBe(`/api/media/${id}.mp4#t=0.001`);
    expect('requestVideoFrameCallback' in video).toBe(false);
    video.dispatchEvent(new Event('loadedmetadata'));
    video.dispatchEvent(new Event('seeked'));
    await grace();
  });
});
