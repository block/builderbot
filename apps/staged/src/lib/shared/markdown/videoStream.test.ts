// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The desktop build, whose video comes from the app's loopback listener
// (src-tauri note_media/stream.rs) rather than the `staged-media` protocol.
vi.mock('../../transport', () => ({ isTauri: true }));
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string, protocol: string) => `${protocol}://localhost/${path}`),
}));
vi.mock('../../commands', () => ({
  getBranchTimeline: vi.fn(),
  listProjectNotes: vi.fn(),
}));

import { listenForReferenceCopy } from '../../features/sessions/referenceClipboard';
import { renderMarkdown } from './renderMarkdown';

const id = '3f2a9c1e-1234-4567-8123-123456789abc';
const other = '3f2a9c1e-1234-4567-8123-123456789abd';
const secret = '5e'.repeat(32);
const stream = `http://127.0.0.1:49152/${secret}`;
const onStream = `${stream}/${id}.mp4#t=0.001`;
const onProtocol = `staged-media://localhost/${id}.mp4#t=0.001`;
/** Where a video that had a place to resume is loaded: no fragment to seek it back. */
const resumedOnProtocol = `staged-media://localhost/${id}.mp4`;
const page = globalThis as { __STAGED_MEDIA_STREAM__?: string };

/** A note as the viewer mounts it; returns its first video. */
function mount(markdown: string): HTMLVideoElement {
  document.body.innerHTML = `<div class="note">${renderMarkdown(markdown)}</div>`;
  return document.querySelector('video')!;
}

/*
 * jsdom has no media pipeline: the test plays WebKit, setting the element's
 * state and firing its events, and reads back what the page did to it.
 */
interface Scripted {
  /** Times the page seeked to, in order. */
  seeks: number[];
  play: ReturnType<typeof vi.fn>;
  state: { paused: boolean; ended: boolean; time: number; error: { code: number } | null };
}

function script(video: HTMLVideoElement, initial: Partial<Scripted['state']> = {}): Scripted {
  const state = { paused: true, ended: false, time: 0, error: null, ...initial };
  const seeks: number[] = [];
  const play = vi.fn(() => {
    state.paused = false;
    return Promise.resolve();
  });
  // Loading a new `src` puts the element back at the start, as the load
  // algorithm does.
  const reset = new MutationObserver(() => void (state.time = 0));
  reset.observe(video, { attributes: true, attributeFilter: ['src'] });
  Object.defineProperties(video, {
    paused: { get: () => state.paused },
    ended: { get: () => state.ended },
    error: { get: () => state.error },
    currentSrc: { get: () => video.getAttribute('src') },
    currentTime: {
      get: () => state.time,
      set: (time: number) => {
        seeks.push(time);
        state.time = time;
      },
    },
    play: { value: play },
  });
  return { seeks, play, state };
}

/** WebKit: the stream failed this video. */
function failStream(video: HTMLVideoElement) {
  video.dispatchEvent(new Event('error'));
}

/**
 * WebKit: the element's current `src` has its metadata (the `src` reset
 * settles first). Then, after the page's listeners, WebKit seeks to the time
 * in the `src`'s fragment when it has one.
 */
async function loaded(video: HTMLVideoElement, clip?: Scripted) {
  await Promise.resolve();
  video.dispatchEvent(new Event('loadedmetadata'));
  const fragment = /#t=([\d.]+)$/.exec(video.getAttribute('src') ?? '');
  if (clip && fragment) clip.state.time = Number(fragment[1]);
}

describe('desktop note video', () => {
  let warn: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    page.__STAGED_MEDIA_STREAM__ = stream;
    warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    delete page.__STAGED_MEDIA_STREAM__;
    document.body.innerHTML = '';
    document.getSelection()?.removeAllRanges();
    warn.mockRestore();
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
    expect(html).toContain(`<video src="${onProtocol}" controls`);
  });

  describe('whose stream fails', () => {
    // The page is told the address before the listener is known to answer.
    it('moves onto the protocol, once', () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      expect(video.getAttribute('src')).toBe(onStream);

      failStream(video);
      expect(video.getAttribute('src')).toBe(onProtocol);

      // A clip that fails there too (a codec WebKit lacks) stays failed.
      failStream(video);
      expect(video.getAttribute('src')).toBe(onProtocol);
    });

    it('moves only the video that failed', () => {
      mount(`![A](staged-media://${id}.mp4)\n\n![B](staged-media://${other}.webm)`);
      const [first, second] = [...document.querySelectorAll('video')];
      failStream(second);
      expect(first.getAttribute('src')).toBe(onStream);
      expect(second.getAttribute('src')).toBe(`staged-media://localhost/${other}.webm#t=0.001`);
    });

    it('leaves videos that are not note media alone', () => {
      mount(`![Clip](staged-media://${id}.mp4)`);
      const src = `${stream}/${id}.mp4`;
      document.body.innerHTML = `<video src="${src}"></video>`;
      const video = document.querySelector('video')!;
      failStream(video);
      expect(video.getAttribute('src')).toBe(src);
    });

    it('says so, without the address', () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      script(video, { error: { code: 2 } });
      failStream(video);
      expect(warn).toHaveBeenCalledTimes(1);
      const [message] = warn.mock.calls[0] as [string];
      expect(message).toContain('media error 2');
      expect(message).toContain(onProtocol);
      expect(message).not.toContain(secret);
      expect(message).not.toContain('127.0.0.1');
    });

    it('in the middle of playback, plays on from where it was', async () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      const clip = script(video, { paused: false, time: 42.5, error: { code: 2 } });

      failStream(video);
      // Without the preview fragment: WebKit's seek to it would come after
      // the resume and put the video back at the start.
      expect(video.getAttribute('src')).toBe(resumedOnProtocol);
      // Asked to play again at once; it starts when the protocol has data.
      expect(clip.play).toHaveBeenCalledTimes(1);
      expect(clip.seeks).toEqual([]);

      await loaded(video, clip);
      expect(clip.seeks).toEqual([42.5]);
      expect(clip.state.time).toBe(42.5);
      expect(clip.state.paused).toBe(false);

      // Only once: a later load of the same element is not a resume.
      await loaded(video, clip);
      expect(clip.seeks).toEqual([42.5]);
      expect(clip.play).toHaveBeenCalledTimes(1);
    });

    it('while paused part way in, stays paused where it was', async () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      const clip = script(video, { time: 17, error: { code: 2 } });

      failStream(video);
      await loaded(video, clip);
      expect(clip.seeks).toEqual([17]);
      expect(clip.state.time).toBe(17);
      expect(clip.play).not.toHaveBeenCalled();
      expect(clip.state.paused).toBe(true);
    });

    it('after it ended, does not start again', async () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      const clip = script(video, { paused: false, ended: true, time: 120 });

      failStream(video);
      await loaded(video);
      expect(clip.play).not.toHaveBeenCalled();
      expect(clip.seeks).toEqual([120]);
    });

    it('before it was ever played, starts over with nothing to resume', async () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      const clip = script(video);

      failStream(video);
      // Nothing to resume, so it keeps the fragment that paints its preview.
      expect(video.getAttribute('src')).toBe(onProtocol);
      await loaded(video, clip);
      expect(clip.seeks).toEqual([]);
      expect(clip.play).not.toHaveBeenCalled();
    });
  });

  /*
   * When the listener dies under a video that already has part of its clip,
   * WebKit fires no `error`: the video waits where its data ends, and after
   * three seconds without any the element fires `stalled`. A healthy load
   * that is ahead of playback fires it too.
   */
  describe('whose stream goes quiet', () => {
    let fetched: ReturnType<typeof vi.fn>;
    /** The listener answers (anything at all) or the connection is refused. */
    const listener = (answers: boolean) => {
      fetched = vi.fn(() => (answers ? Promise.resolve({}) : Promise.reject(new TypeError())));
      vi.stubGlobal('fetch', fetched);
    };
    const stall = (video: HTMLVideoElement) => video.dispatchEvent(new Event('stalled'));
    const settled = () => new Promise((resolve) => setTimeout(resolve));

    afterEach(() => vi.unstubAllGlobals());

    it('asks the listener whether it is there, naming neither the secret nor a video', async () => {
      listener(true);
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      script(video, { paused: false, time: 31 });

      stall(video);
      await settled();
      expect(fetched).toHaveBeenCalledTimes(1);
      expect(fetched).toHaveBeenCalledWith('http://127.0.0.1:49152', {
        mode: 'no-cors',
        cache: 'no-store',
      });
      // It answered: the stall is the load being ahead of playback.
      expect(video.getAttribute('src')).toBe(onStream);
      expect(warn).not.toHaveBeenCalled();
    });

    it('plays on over the protocol from where it was when the listener is gone', async () => {
      listener(false);
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      const clip = script(video, { paused: false, time: 31 });

      stall(video);
      await settled();
      expect(video.getAttribute('src')).toBe(resumedOnProtocol);
      expect(clip.play).toHaveBeenCalledTimes(1);
      const [message] = warn.mock.calls[0] as [string];
      expect(message).toContain('stopped answering');
      expect(message).not.toContain(secret);
      expect(message).not.toContain('127.0.0.1');

      await loaded(video, clip);
      expect(clip.seeks).toEqual([31]);
      expect(clip.state.time).toBe(31);
      expect(clip.state.paused).toBe(false);
      // The listener is gone for every other video too.
      expect(page.__STAGED_MEDIA_STREAM__).toBeUndefined();
    });

    it('stays paused where it was when the listener is gone', async () => {
      listener(false);
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      const clip = script(video, { time: 12 });

      stall(video);
      await settled();
      await loaded(video, clip);
      expect(video.getAttribute('src')).toBe(resumedOnProtocol);
      expect(clip.seeks).toEqual([12]);
      expect(clip.state.time).toBe(12);
      expect(clip.play).not.toHaveBeenCalled();
    });

    it('leaves alone a video that was closed while the question was out', async () => {
      listener(false);
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      script(video, { paused: false, time: 31 });

      stall(video);
      video.remove();
      await settled();
      expect(video.getAttribute('src')).toBe(onStream);
      expect(warn).not.toHaveBeenCalled();
    });

    it('asks nothing about a video that is not on the stream', async () => {
      listener(false);
      delete page.__STAGED_MEDIA_STREAM__;
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      script(video);

      stall(video);
      await settled();
      expect(fetched).not.toHaveBeenCalled();
      expect(video.getAttribute('src')).toBe(onProtocol);
    });
  });

  // With a dead listener every new video would start with a failed load.
  describe('once the protocol has loaded what the stream did not', () => {
    it('renders later videos on the protocol and says why', async () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      failStream(video);
      // Not yet: the clip itself may be the problem.
      expect(renderMarkdown(`![B](staged-media://${other}.mp4)`)).toContain(`src="${stream}/`);

      await loaded(video);
      expect(page.__STAGED_MEDIA_STREAM__).toBeUndefined();
      const later = renderMarkdown(`![B](staged-media://${other}.mp4)`);
      expect(later).toContain(`<video src="staged-media://localhost/${other}.mp4#t=0.001"`);
      expect(later).not.toContain('127.0.0.1');
      expect(warn).toHaveBeenCalledTimes(2);
      const [message] = warn.mock.calls[1] as [string];
      expect(message).toContain('not answering');
      expect(message).not.toContain(secret);
    });

    it('says why once, however many videos were on the stream', async () => {
      mount(`![A](staged-media://${id}.mp4)\n\n![B](staged-media://${other}.webm)`);
      const [first, second] = [...document.querySelectorAll('video')];
      failStream(first);
      failStream(second);
      await loaded(first);
      await loaded(second);
      // One line per video moved, one for the stream.
      expect(warn).toHaveBeenCalledTimes(3);
    });

    it('keeps the stream when the clip fails on the protocol too', () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      failStream(video);
      failStream(video);
      expect(page.__STAGED_MEDIA_STREAM__).toBe(stream);
      expect(renderMarkdown(`![B](staged-media://${other}.mp4)`)).toContain(`src="${stream}/`);
    });

    it('keeps the stream when the clip could not be decoded', async () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      const clip = script(video, { paused: false, time: 9, error: { code: 3 } });
      failStream(video);
      await loaded(video);
      // Moved and resumed like any other, but the bytes had arrived.
      expect(clip.seeks).toEqual([9]);
      expect(page.__STAGED_MEDIA_STREAM__).toBe(stream);
    });

    it('is not misled by a video that loads without having failed', async () => {
      const video = mount(`![Clip](staged-media://${id}.mp4)`);
      await loaded(video);
      expect(page.__STAGED_MEDIA_STREAM__).toBe(stream);
      expect(warn).not.toHaveBeenCalled();
    });
  });

  /*
   * The address holds the secret that gates every stored video, so it must
   * not leave the page. WebKit's own copy and drag serialize a selected
   * `<video>` with its `src`.
   */
  describe('keeps the stream address in the page', () => {
    type Flavours = Record<string, string>;

    /** jsdom has no ClipboardEvent/DragEvent/DataTransfer; an event with a recording one. */
    function transferEvent(type: 'copy' | 'dragstart'): { event: Event; data: Flavours } {
      const data: Flavours = {};
      const event = new Event(type, { bubbles: true, cancelable: true });
      Object.defineProperty(event, type === 'copy' ? 'clipboardData' : 'dataTransfer', {
        value: { setData: (flavour: string, value: string) => void (data[flavour] = value) },
      });
      return { event, data };
    }

    function selectContents(node: Node) {
      const range = document.createRange();
      range.selectNodeContents(node);
      const selection = document.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);
    }

    const note = () => document.querySelector('.note')!;
    const withVideo = `Before the clip.\n\n![Clip](staged-media://${id}.mp4)\n\nAfter it.`;

    it('copies a selection that takes in a video with the protocol URL instead', () => {
      mount(withVideo);
      selectContents(note());
      const { event, data } = transferEvent('copy');
      note().dispatchEvent(event);

      expect(event.defaultPrevented).toBe(true);
      expect(data['text/html']).toMatch(/<video [^>]*src="staged-media:\/\/localhost\/[^"]+"/);
      expect(data['text/html']).toContain(`src="${onProtocol}"`);
      expect(data['text/html']).toContain('<p>Before the clip.</p>');
      expect(data['text/html']).toContain(`data-media-id="${id}"`);
      expect(data['text/plain']).toContain('Before the clip.');
      expect(data['text/plain']).toContain('After it.');
      for (const flavour of Object.values(data)) {
        expect(flavour).not.toContain(secret);
        expect(flavour).not.toContain('127.0.0.1');
      }
      // The video on the page is untouched.
      expect(document.querySelector('video')!.getAttribute('src')).toBe(onStream);
    });

    it('covers every video in the selection and every range', () => {
      mount(`![A](staged-media://${id}.mp4)\n\ntext\n\n![B](staged-media://${other}.webm)`);
      const selection = document.getSelection()!;
      selection.removeAllRanges();
      const [first, second] = [...document.querySelectorAll('figure')];
      for (const figure of [first, second]) {
        const range = document.createRange();
        range.selectNode(figure);
        selection.addRange(range);
      }
      const { event, data } = transferEvent('copy');
      note().dispatchEvent(event);
      expect(data['text/html']).toContain(onProtocol);
      // jsdom keeps one range; WebKit, too. Either way nothing of the stream.
      expect(data['text/html']).not.toContain(secret);

      selectContents(note());
      const whole = transferEvent('copy');
      note().dispatchEvent(whole.event);
      expect(whole.data['text/html']).toContain(onProtocol);
      expect(whole.data['text/html']).toContain(`staged-media://localhost/${other}.webm#t=0.001`);
      expect(whole.data['text/html']).not.toContain(secret);
    });

    it('leaves a copy with no video in it to the browser', () => {
      mount(withVideo);
      selectContents(note().querySelector('p')!);
      const { event, data } = transferEvent('copy');
      note().dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
      expect(data).toEqual({});
    });

    it('leaves a copy of a video that is on the protocol to the browser', () => {
      const video = mount(withVideo);
      failStream(video);
      selectContents(note());
      const { event, data } = transferEvent('copy');
      note().dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
      expect(data).toEqual({});
    });

    it('leaves editors to write their own clipboard', () => {
      mount(withVideo);
      note().setAttribute('contenteditable', 'true');
      note().querySelector('figure')!.setAttribute('contenteditable', 'false');
      selectContents(note());
      const { event, data } = transferEvent('copy');
      note().dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
      expect(data).toEqual({});
    });

    // The app's reference copy runs first and writes the clipboard itself,
    // with no `<video>` in it at all.
    it('leaves a selection with a reference badge to the reference copy', () => {
      const unlisten = listenForReferenceCopy();
      try {
        mount(withVideo);
        const badge = document.createElement('span');
        badge.setAttribute('data-staged-ref', '#note:b7c40af7-ca85-4b29-98f3-4728a508de83');
        badge.textContent = 'Porting notes';
        note().querySelector('p')!.append(' ', badge);
        selectContents(note());
        const { event, data } = transferEvent('copy');
        note().dispatchEvent(event);
        expect(event.defaultPrevented).toBe(true);
        expect(data['text/html']).toContain('data-staged-copy');
        expect(data['text/html']).not.toContain('<video');
        expect(data['text/html']).not.toContain(secret);
      } finally {
        unlisten();
      }
    });

    it('puts the same on a drag of the selection, without cancelling it', () => {
      mount(withVideo);
      selectContents(note());
      const { event, data } = transferEvent('dragstart');
      note().querySelector('p')!.firstChild!.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
      expect(data['text/html']).toMatch(/<video [^>]*src="staged-media:\/\/localhost\/[^"]+"/);
      expect(data['text/html']).toContain(`src="${onProtocol}"`);
      expect(data['text/html']).not.toContain(secret);
      expect(data['text/plain']).toContain('Before the clip.');
    });

    it('leaves a drag of something outside the selection alone', () => {
      mount(withVideo);
      const outside = document.createElement('p');
      outside.textContent = 'elsewhere';
      document.body.append(outside);
      selectContents(note());
      const { event, data } = transferEvent('dragstart');
      outside.dispatchEvent(event);
      expect(data).toEqual({});
    });

    // WebKit's menu for a video offers to copy, download and open its address.
    it('shows no native menu for a video on the stream', () => {
      const video = mount(withVideo);
      const onStreamMenu = new Event('contextmenu', { bubbles: true, cancelable: true });
      video.dispatchEvent(onStreamMenu);
      expect(onStreamMenu.defaultPrevented).toBe(true);

      const elsewhere = new Event('contextmenu', { bubbles: true, cancelable: true });
      note().querySelector('p')!.dispatchEvent(elsewhere);
      expect(elsewhere.defaultPrevented).toBe(false);

      // On the protocol the address is inert, and the menu stays.
      failStream(video);
      const onProtocolMenu = new Event('contextmenu', { bubbles: true, cancelable: true });
      video.dispatchEvent(onProtocolMenu);
      expect(onProtocolMenu.defaultPrevented).toBe(false);
    });
  });
});
