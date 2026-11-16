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
 *    (`leaveStalledStream`) moves onto the protocol, where it resumes. The
 *    page stops using the stream only when the listener itself does not
 *    answer: one failed load on a listener that is still there costs that
 *    one video its stream and no other.
 * 2. It is a secret. Anything that reads the secret can ask the listener for
 *    any stored video until the app restarts, so the address must not leave
 *    the page. `keepStreamAddressInApp` covers the ways WebKit would hand it
 *    out: a copy or a drag of a selection that takes in a video, which
 *    serializes the `src`, and the video's context menu, which offers to copy
 *    the address and to open, download or share it.
 */
import { elementOf, isEditable } from '../../features/sessions/referenceClipboard';
import { abandonVideoStream, protocolUrlForStreamedVideo } from './mediaUrl';

/**
 * Seconds. A video no further in than this has no place to resume. It is at
 * the start of its clip, where the preview fragment (`videoPreview.ts`) put
 * it, or on one of the times the preview helper there holds it on while it
 * looks for a frame to paint, which is not somewhere the reader went. The
 * furthest of those is 1 s, reported to within a millisecond. A reader who
 * really was this far in starts over, a second back at most.
 */
const AT_START = 1.001;

/** Where each video that is on its way off the stream was, in seconds. */
const leftStream = new WeakMap<HTMLVideoElement, number>();

/** The address a video is loading from. */
function sourceOf(video: HTMLVideoElement): string {
  return video.currentSrc || video.getAttribute('src') || '';
}

/** The protocol URL for a video that is on the stream; null for any other. */
function offStream(video: HTMLVideoElement): string | null {
  return protocolUrlForStreamedVideo(sourceOf(video));
}

/**
 * Called for a note video's `error`. A video on the stream is loaded over the
 * `staged-media` protocol instead, which is slower to seek but needs nothing
 * beyond the app itself. This happens once per video: the protocol URL is not
 * a stream URL, so a clip WebKit cannot play at all fails a second time and
 * stays failed.
 *
 * Whether later videos still use the stream is the listener's to say, not the
 * error's. A listener that is there can fail one load (a file that is moving
 * with its project, a dropped connection), and a dead one fails them all.
 */
export function leaveFailedStream(video: HTMLVideoElement): void {
  const src = sourceOf(video);
  if (!offStream(video)) return;
  leaveStream(video, `failed (media error ${video.error?.code ?? 'unknown'})`);
  ifListenerGone(src);
}

/**
 * Called for a note video's `stalled`: it is loading and nothing has arrived
 * for three seconds. That is all WebKit reports when the listener dies under
 * a video that already has part of its clip. There is no `error`; the video
 * waits where its data ends, for good. A healthy load that is ahead of
 * playback stalls too, so the video leaves the stream, as a failed one does,
 * only when the listener is gone.
 */
export function leaveStalledStream(video: HTMLVideoElement): void {
  const src = video.currentSrc;
  if (!offStream(video)) return;
  ifListenerGone(src, () => {
    // Unless something else has moved or removed the video in the meantime.
    if (video.isConnected && video.currentSrc === src) leaveStream(video, 'stopped answering');
  });
}

/**
 * Ask the listener behind a stream URL whether it still answers, with a
 * request that names neither the secret nor a video. Any answer means it is
 * there. When the connection is refused it is gone for every video, so the
 * page stops using the stream (later videos render on the protocol instead
 * of each starting with a failed load) and `then` runs. A window that is
 * reloaded asks the app again.
 */
function ifListenerGone(src: string, then?: () => void): void {
  fetch(new URL(src).origin, { mode: 'no-cors', cache: 'no-store' }).catch(() => {
    then?.();
    if (abandonVideoStream()) {
      console.warn(
        '[note video] the video stream is not answering; note video uses staged-media until this window reloads'
      );
    }
  });
}

/**
 * Move a video that is on the stream to the protocol. A video that was
 * playing plays on, and `resumeAfterLeavingStream` puts it back where it was.
 */
function leaveStream(video: HTMLVideoElement, why: string): void {
  const onProtocol = offStream(video);
  // Once per video. WebKit's `currentSrc` still names the stream for a task
  // after the new `src` is set, and a second pass in that time would save the
  // rewound element's position over the real one.
  if (!onProtocol || leftStream.has(video)) return;
  // Both read before the new `src`: loading it pauses and rewinds the element.
  const wasPlaying = !video.paused && !video.ended;
  const at = video.currentTime;
  leftStream.set(video, at);
  // A video with a place to resume loads without its preview fragment.
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
 * stream, this is the protocol loading what the stream did not, and the video
 * goes back to where it was. A seek from script, not a `#t=` fragment: WebKit
 * paints nothing for a fragment's seek on some MP4s (see `videoPreview.ts`),
 * which would leave a paused video blank. `leaveStream` loaded it with no
 * fragment at all, so nothing seeks after this.
 */
export function resumeAfterLeavingStream(video: HTMLVideoElement): void {
  const at = leftStream.get(video);
  leftStream.delete(video);
  if (at !== undefined && at > AT_START) video.currentTime = at;
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
 * Write the selection to `data`, with every video that is on the stream named
 * by its protocol URL instead, which is what a copy held before there was a
 * stream. This is plainer than the browser's own copy, which it replaces: the
 * markup of the selected nodes as they are in the page, without the computed
 * styles the browser inlines, without the elements the selection sits inside
 * (some of a list's items are copied without their `ul`), and with no web
 * archive flavour. Only a selection that takes in a video on the stream pays
 * that.
 *
 * Returns false, having written nothing, when the selection takes in no such
 * video or is inside an editor: an editor writes its own clipboard from the
 * document, where a video is its `staged-media://` reference. With `from`,
 * also when the selection does not reach that node.
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
