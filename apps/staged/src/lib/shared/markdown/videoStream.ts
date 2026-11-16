/**
 * videoStream.ts — the page's side of the desktop app's loopback video stream
 *
 * On desktop a note `<video>` loads from `http://127.0.0.1:<port>/<secret>/…`
 * (src-tauri `note_media/stream.rs`, `resolveStagedVideoUrl`). That address
 * differs from the `staged-media` protocol URL it replaced in two ways, and
 * this module is what the page does about each. `videoPreview.ts` wires both
 * to the document.
 *
 * 1. It can stop answering. The app names it before knowing that it will
 *    serve, and a listener can die. A video whose stream failed
 *    (`leaveFailedStream`) or went quiet because the listener is gone
 *    (`leaveStalledStream`) moves onto the protocol, where it resumes, and
 *    once the protocol has loaded what the stream could not, the page stops
 *    using the stream.
 * 2. It is a secret. Anything that reads the secret can ask the listener for
 *    any stored video until the app restarts, so the address must not leave
 *    the page. `keepStreamAddressInApp` covers the ways WebKit would hand it
 *    out: a copy or a drag of a selection that takes in a video, which
 *    serializes the `src`, and the video's context menu, which offers to copy
 *    the address and to open, download or share it.
 */
import { abandonVideoStream, protocolUrlForStreamedVideo } from './mediaUrl';

/** `MediaError.MEDIA_ERR_DECODE`: the bytes arrived and could not be decoded. */
const MEDIA_ERR_DECODE = 3;

/**
 * Seconds. A video no further in than this is at the start of its clip, where
 * the preview fragment (`videoPreview.ts`) put it, and has no place to resume.
 */
const AT_START = 0.01;

/** How a video was doing when it left the stream. */
interface LeftStream {
  /** Where it was, in seconds. */
  at: number;
  /** Whether the failure could have been the listener's rather than the clip's. */
  streamSuspected: boolean;
}

const leftStream = new WeakMap<HTMLVideoElement, LeftStream>();

/** The protocol URL for a video that is on the stream; null for any other. */
function offStream(video: HTMLVideoElement): string | null {
  return protocolUrlForStreamedVideo(video.currentSrc || video.getAttribute('src') || '');
}

/**
 * Called for a note video's `error`. A video on the stream is loaded over the
 * `staged-media` protocol instead, which is slower to seek but needs nothing
 * beyond the app itself. This happens once per video: the protocol URL is not
 * a stream URL, so a clip WebKit cannot play at all fails a second time and
 * stays failed.
 */
export function leaveFailedStream(video: HTMLVideoElement): void {
  const code = video.error?.code;
  leaveStream(video, `failed (media error ${code ?? 'unknown'})`, code !== MEDIA_ERR_DECODE);
}

/**
 * Called for a note video's `stalled`: it is loading and nothing has arrived
 * for three seconds. That is all WebKit reports when the listener dies under
 * a video that already has part of its clip. There is no `error`; the video
 * waits where its data ends, for good. So a stall asks whether the listener
 * still answers, and a video whose listener is gone leaves the stream as a
 * failed one does. Any answer means it is there (a stall is also what a
 * healthy load that is ahead of playback looks like), and the request names
 * neither the secret nor a video.
 */
export function leaveStalledStream(video: HTMLVideoElement): void {
  const src = video.currentSrc;
  if (!offStream(video)) return;
  fetch(new URL(src).origin, { mode: 'no-cors', cache: 'no-store' }).catch(() => {
    // Unless something else has moved or removed the video in the meantime.
    if (video.isConnected && video.currentSrc === src) {
      leaveStream(video, 'stopped answering', true);
    }
  });
}

/**
 * Move a video that is on the stream to the protocol. A video that was
 * playing plays on, and `resumeAfterLeavingStream` puts it back where it was.
 */
function leaveStream(video: HTMLVideoElement, why: string, streamSuspected: boolean): void {
  const onProtocol = offStream(video);
  if (!onProtocol) return;
  // Both read before the new `src`: loading it pauses and rewinds the element.
  const wasPlaying = !video.paused && !video.ended;
  const at = video.currentTime;
  leftStream.set(video, { at, streamSuspected });
  // A video that had left the start loads without its preview fragment.
  // WebKit seeks to a fragment's time after `loadedmetadata`, which would
  // undo the resume.
  const fallback = at > AT_START ? onProtocol.split('#')[0] : onProtocol;
  // The protocol URL carries no secret, so it can be logged.
  console.warn(`[note video] stream ${why}, loading ${fallback} instead`);
  video.src = fallback;
  // Rejects when the protocol cannot play it either; the controls show that.
  if (wasPlaying) video.play().catch(() => {});
}

/**
 * Called for a note video's `loadedmetadata`. For a video that left the
 * stream, this is the protocol loading what the stream did not:
 *
 * - The video goes back to where it was. A seek from script, not a `#t=`
 *   fragment: WebKit paints nothing for a fragment's seek on some MP4s (see
 *   `videoPreview.ts`), which would leave a paused video blank. `leaveStream`
 *   loaded it with no fragment at all, so nothing seeks after this.
 * - Unless the clip itself was at fault (a decode error), the stream is, so
 *   the page stops using it: every later video renders on the protocol
 *   instead of paying for a failed load first. A window that is reloaded
 *   asks the app again.
 */
export function resumeAfterLeavingStream(video: HTMLVideoElement): void {
  const left = leftStream.get(video);
  if (!left) return;
  leftStream.delete(video);
  if (left.at > AT_START) video.currentTime = left.at;
  if (left.streamSuspected && abandonVideoStream()) {
    console.warn(
      '[note video] the video stream is not answering; note video uses staged-media until this window reloads'
    );
  }
}

let keeping = false;

/**
 * Keep the stream address inside the page (point 2 above). The listeners are
 * on the document, in the bubble phase, so they run after the app's own copy
 * handling (`listenForReferenceCopy`, which writes the clipboard itself and
 * leaves out every `<video>`), and they do nothing where no video on the
 * stream is involved: on the web, on the protocol, and for any other copy.
 */
export function keepStreamAddressInApp(): void {
  if (keeping || typeof document === 'undefined') return;
  keeping = true;
  document.addEventListener('copy', (event) => {
    if (event.defaultPrevented || !event.clipboardData) return;
    // The browser writes nothing of its own once the event is cancelled.
    if (writeSelectionWithoutStream(event.clipboardData)) event.preventDefault();
  });
  document.addEventListener('dragstart', (event) => {
    // Only a drag that starts in the selection carries its markup, and
    // cancelling the event would cancel the drag. WebKit adds nothing of its
    // own to a drag that the page has put data on.
    if (event.defaultPrevented || !event.dataTransfer || !(event.target instanceof Node)) return;
    writeSelectionWithoutStream(event.dataTransfer, event.target);
  });
  // WebKit's menu for a video has Copy Video Address, Open Video in New
  // Window, Download Video and Share…, each of which hands the address on.
  // Of its other entries (play, mute, full screen, picture in picture) all
  // but Loop are in the controls.
  document.addEventListener('contextmenu', (event) => {
    if (event.target instanceof HTMLVideoElement && offStream(event.target)) {
      event.preventDefault();
    }
  });
}

/**
 * Write the selection to `data` as the browser would have, with every video
 * that is on the stream named by its protocol URL instead, which is what a
 * copy held before there was a stream. Returns false, having written
 * nothing, when the selection takes in no such video or is inside an editor:
 * an editor writes its own clipboard from the document, where a video is its
 * `staged-media://` reference. With `from`, also when the selection does not
 * reach that node.
 */
function writeSelectionWithoutStream(data: DataTransfer, from?: Node): boolean {
  const selection = document.getSelection();
  if (!selection || selection.isCollapsed) return false;
  const ranges = Array.from({ length: selection.rangeCount }, (_, i) => selection.getRangeAt(i));
  if (from && !ranges.some((range) => range.intersectsNode(from))) return false;
  if (ranges.some(isEditable) || !ranges.some(holdsStreamedVideo)) return false;
  data.setData('text/html', ranges.map(htmlWithoutStream).join(''));
  data.setData('text/plain', selection.toString());
  return true;
}

function elementOf(node: Node): Element | null {
  return node instanceof Element ? node : node.parentElement;
}

function isEditable(range: Range): boolean {
  return !!elementOf(range.commonAncestorContainer)?.closest(
    '[contenteditable]:not([contenteditable="false"])'
  );
}

function holdsStreamedVideo(range: Range): boolean {
  const root = elementOf(range.commonAncestorContainer);
  if (!root) return false;
  for (const video of root.querySelectorAll('video')) {
    if (range.intersectsNode(video) && offStream(video)) return true;
  }
  return false;
}

/**
 * The markup inside `range`, with stream addresses replaced. A cloned
 * `<video>` would start loading its `src`, so the clone loses it at once,
 * and the replacement is put on in a document that loads nothing.
 */
function htmlWithoutStream(range: Range): string {
  const copy = range.cloneContents();
  const sources = Array.from(copy.querySelectorAll('video'), (video) => {
    const src = video.getAttribute('src');
    video.removeAttribute('src');
    return src;
  });
  const inert = document.implementation.createHTMLDocument('');
  const holder = inert.createElement('div');
  holder.append(inert.importNode(copy, true));
  holder.querySelectorAll('video').forEach((video, i) => {
    const src = sources[i];
    if (src !== null) video.setAttribute('src', protocolUrlForStreamedVideo(src) ?? src);
  });
  return holder.innerHTML;
}
