/**
 * videoPreview.ts — a note video's `src`, and its first frame before it is played
 *
 * `resolveStagedVideoSrc` is where the viewer (`mediaRendering.ts`) and the
 * editor (`wysiwygMedia.ts`) get a `<video src>`: the served URL
 * (`resolveStagedVideoUrl`, the desktop app's loopback stream when it has
 * one) with the preview fragment below. It also starts the document-wide
 * watch over note videos, which paints the preview and does the page's part
 * for that stream (`videoStream.ts`): a video whose stream fails is put back
 * on the `staged-media` protocol, and the stream's address stays in the page.
 *
 * WebKit (the desktop WKWebView, Safari, every iOS browser) decodes nothing
 * for `preload="metadata"`, so a note video is an empty box until played.
 * Chromium paints the first frame on its own. Two steps fix that, and the
 * viewer (`mediaRendering.ts`) and the editor (`wysiwygMedia.ts`) get both by
 * taking their `<video src>` from `resolveStagedVideoSrc`:
 *
 * 1. A `#t=` media fragment on the `src`. WebKit seeks to it once the
 *    metadata is in, which decodes and paints that frame. This is enough for
 *    WebM, and for MP4 and MOV whose video track starts at zero.
 * 2. A script seek for the rest. An MP4 or MOV written without an edit list
 *    has a video track that starts a frame or two in, and for those WebKit
 *    paints nothing for the fragment's seek, whatever time it names. A seek
 *    from script to a time that has a frame does paint, so when the
 *    fragment's seek presented no frame, later times are tried in turn.
 *
 * Neither step touches how media is served: a fragment never reaches the
 * server, and the script seeks are ordinary ranged reads. Both were measured
 * in a WKWebView; jsdom has no media pipeline to show either.
 *
 * Step 2 asks for a frame callback, and WebKit stops drawing a video that
 * was created in another document once one has been asked for. Markup that
 * goes in with `{@html}` is created that way, so those render sites swap the
 * video for one the page made (pageVideos.ts).
 */
import { resolveStagedVideoUrl } from './mediaUrl';
import {
  keepStreamAddressInApp,
  leaveFailedStream,
  leaveStalledStream,
  resumeAfterLeavingStream,
} from './videoStream';

/**
 * Start time of the media fragment, in seconds. Small enough to be the first
 * frame of a clip, and not zero: WebKit does not seek for `#t=0`.
 */
const PREVIEW_TIME = '0.001';

/** Times tried, in order, when the fragment's seek presented no frame. */
const RETRY_TIMES = [0.05, 0.1, 0.25, 0.5, 1];

/**
 * How long a finished seek is given to present its frame. WKWebView takes 10
 * to 20 ms; a wrong "nothing shown" under load only costs two extra seeks.
 */
const PAINT_GRACE_MS = 250;

/**
 * How far past one of the times above a video can have played by the time
 * its `play` event is handled, in seconds.
 */
const PLAY_DRIFT = 0.5;

/** `src` for a note `<video>`: the served URL, with the preview fragment. */
export function resolveStagedVideoSrc(id: string, ext: string): string {
  watchNoteVideos();
  return withVideoPreviewFragment(resolveStagedVideoUrl(id, ext));
}

/**
 * Add the preview fragment to a served video URL. A query string stays in
 * front of it, a fragment that already names a time is left alone, and any
 * other fragment keeps its fields and gains the time (media fragments are
 * `&`-separated name=value pairs).
 */
export function withVideoPreviewFragment(url: string): string {
  const hash = url.indexOf('#');
  if (hash === -1) return `${url}#t=${PREVIEW_TIME}`;
  const fragment = url.slice(hash + 1);
  if (/(^|&)t=/.test(fragment)) return url;
  return `${url}${fragment ? '&' : ''}t=${PREVIEW_TIME}`;
}

let watching = false;

/**
 * Media events do not bubble, but a capturing listener on the document sees
 * them for every note video, whichever markup path drew it and however it was
 * mounted (`{@html}`, a ProseMirror node view).
 */
function watchNoteVideos() {
  if (watching || typeof document === 'undefined') return;
  watching = true;
  const onNoteVideo = (type: string, handle: (video: HTMLVideoElement) => void) =>
    document.addEventListener(
      type,
      (event) => {
        const video = event.target;
        if (video instanceof HTMLVideoElement && video.closest('.markdown-media-video')) {
          handle(video);
        }
      },
      true
    );
  onNoteVideo('loadedmetadata', (video) => {
    // First: a video resumed after leaving the stream is not at the start,
    // and the preview below then leaves it alone.
    resumeAfterLeavingStream(video);
    void paintBlankPreview(video);
  });
  onNoteVideo('error', leaveFailedStream);
  onNoteVideo('stalled', leaveStalledStream);
  keepStreamAddressInApp();
}

/**
 * Step 2 above. Runs from `loadedmetadata`, which is before the fragment's
 * seek has finished, so a frame that seek presents is not missed. Once a
 * later time has painted, the video is put back on the fragment's time:
 * WebKit keeps the painted frame up, and playback and the timeline still
 * start at the beginning.
 *
 * While it tries the later times the video really is at one of them. A
 * reader who presses play then (or Space in the editor) must still get the
 * clip from its start, so for as long as this runs a `play` on one of its own
 * times goes back there first.
 */
async function paintBlankPreview(video: HTMLVideoElement) {
  if (typeof video.requestVideoFrameCallback !== 'function') return;
  let painted = false;
  video.requestVideoFrameCallback(() => {
    painted = true;
  });
  const src = video.currentSrc;
  await once(video, 'seeked');
  const start = video.currentTime;
  // Only a video at the start of its clip is waiting for a preview. One that
  // was resumed further in shows the frame it was resumed on.
  if (!(start < RETRY_TIMES[0])) return;
  let at = start;
  let playing = false;
  const playFromStart = () => {
    if (playing) return;
    playing = true;
    const ahead = video.currentTime - at;
    // Still on this helper's time, give or take what has played since. A
    // reader who scrubbed first is somewhere else and stays there.
    if (at !== start && video.currentSrc === src && ahead > -0.001 && ahead < PLAY_DRIFT) {
      video.currentTime = start;
    }
  };
  video.addEventListener('play', playFromStart);
  try {
    for (const time of [...RETRY_TIMES, null]) {
      await paintGrace();
      // The `play` event may still be on its way.
      if (!video.paused) playFromStart();
      // Played, scrubbed, reloaded or removed: the video is the reader's now.
      const untouched =
        !playing &&
        video.isConnected &&
        !video.seeking &&
        video.currentSrc === src &&
        Math.abs(video.currentTime - at) < 0.001;
      if (!untouched) return;
      if (painted || time === null || !(time < video.duration)) break;
      at = time;
      video.currentTime = time;
      await once(video, 'seeked');
    }
    if (at !== start) video.currentTime = start;
  } finally {
    video.removeEventListener('play', playFromStart);
  }
}

/** A hidden page presents no frames, so the wait only starts once it shows. */
async function paintGrace() {
  while (document.hidden) await once(document, 'visibilitychange');
  await new Promise((resolve) => setTimeout(resolve, PAINT_GRACE_MS));
}

function once(target: EventTarget, type: string): Promise<void> {
  return new Promise((resolve) => target.addEventListener(type, () => resolve(), { once: true }));
}
