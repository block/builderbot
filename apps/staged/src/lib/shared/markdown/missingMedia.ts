/**
 * missingMedia.ts — a placeholder for media a rendered text names and the
 * store no longer has
 *
 * A note's media is deleted with the last text that embeds it. A revision
 * that drops or replaces an image deletes its file as it saves
 * (`cleanup_unreferenced` in src-tauri, which reads note bodies and chat,
 * and not the versions a note keeps). The note's previous version still
 * names the image, and rendered as it was written it shows whatever the
 * browser draws for a source that does not load: a broken-image glyph, or a
 * video's controls with nothing behind them, at the very block a change
 * marker points to.
 *
 * So where a text can outlive its media, the element `{@html}` fills takes
 *
 *     {@attach missingMedia(html, isKept)}
 *
 * and a figure whose image or video fails to load is marked
 * `data-media-missing`, which mediaStyles.css draws as a box that says the
 * image, or the video, is no longer available. The box has a size of its
 * own. The size of the media went with its row, and a fixed one is laid out
 * once and stays, where a failed image is a few pixels of glyph or a line of
 * alt text, as the browser sees fit.
 *
 * Only attributes are set. The figure keeps its children, hidden, and gains
 * none: the note diff reads a version's blocks from this markup
 * (noteDiffUnits.ts) and marks them only while they read as the blocks it
 * compared.
 *
 * What fails to load is not always gone. A video the browser cannot play
 * fails too, and so does anything when the app is unreachable. `isKept`
 * names the media known to be there (the ids the note itself embeds), and a
 * failure of one of those is left to show as it does in the note.
 */
import type { Attachment } from 'svelte/attachments';

/** A media figure's image or video, as mediaRendering.ts writes them. */
const MEDIA_ELEMENT = '.markdown-media[data-media-id] > :is(img, video)';

const LABELS = {
  image: 'Image no longer available',
  video: 'Video no longer available',
} as const;

function markMissing(media: HTMLImageElement | HTMLVideoElement): void {
  const figure = media.parentElement;
  if (!figure || figure.hasAttribute('data-media-missing')) return;
  const kind = media instanceof HTMLVideoElement ? 'video' : 'image';
  figure.setAttribute('data-media-missing', kind);
  figure.setAttribute('data-media-missing-label', LABELS[kind]);
}

/**
 * Watch the media figures under `root`, and mark each one whose image or
 * video fails to load, unless `isKept` says its id is still there. Returns
 * the undo for the watching; a figure that was marked stays marked.
 *
 * A video's failure is not always its last word. On the desktop app's
 * loopback stream it is loaded again over the `staged-media` protocol when
 * the stream fails (`leaveFailedStream` in videoStream.ts, which hears the
 * `error` first, from the document). So a video is marked only when the
 * source it has after failing is the one it failed with: one that was given
 * another to try is heard from again.
 */
export function watchForMissingMedia(
  root: HTMLElement,
  isKept: (id: string) => boolean = () => false
): () => void {
  /** The source each video was last seen to start loading. */
  const loading = new WeakMap<HTMLVideoElement, string | null>();
  const givenAnotherSource = (video: HTMLVideoElement): boolean => {
    const now = video.getAttribute('src');
    const failed = loading.has(video) ? loading.get(video) : now;
    loading.set(video, now);
    return failed !== now;
  };

  const consider = (media: Element) => {
    if (!media.matches(MEDIA_ELEMENT)) return;
    if (media instanceof HTMLVideoElement && givenAnotherSource(media)) return;
    const id = media.parentElement?.getAttribute('data-media-id');
    if (!id || isKept(id)) return;
    markMissing(media as HTMLImageElement | HTMLVideoElement);
  };
  // Neither event bubbles: they are caught on their way down.
  const onError = (event: Event) => {
    if (event.target instanceof Element) consider(event.target);
  };
  const onLoadStart = ({ target }: Event) => {
    if (target instanceof HTMLVideoElement) loading.set(target, target.getAttribute('src'));
  };
  root.addEventListener('error', onError, true);
  root.addEventListener('loadstart', onLoadStart, true);

  for (const video of root.querySelectorAll<HTMLVideoElement>(`${MEDIA_ELEMENT}:is(video)`)) {
    loading.set(video, video.getAttribute('src'));
  }
  for (const image of root.querySelectorAll<HTMLImageElement>(`${MEDIA_ELEMENT}:is(img)`)) {
    // Asked for now, wherever it is in the text: an image that waited until
    // it was scrolled near would turn into its placeholder under the reader,
    // and move everything after it.
    image.setAttribute('loading', 'eager');
    // One that failed before this ran has no event left to send.
    if (image.complete && image.naturalWidth === 0 && image.getAttribute('src')) consider(image);
  }
  return () => {
    root.removeEventListener('error', onError, true);
    root.removeEventListener('loadstart', onLoadStart, true);
  };
}

/**
 * For the element `{@html html}` fills. Svelte runs an attachment again when
 * it is handed a different function, so each render of the markup is watched
 * from the start, and the watch on the one before it ends.
 */
export function missingMedia(
  _html: string | null,
  isKept?: (id: string) => boolean
): Attachment<HTMLElement> {
  return (node) => watchForMissingMedia(node, isKept);
}
