/**
 * pageVideos.ts — a rendered note video, as an element the page itself made
 *
 * Svelte fills `{@html}` for the first time while the element around it is
 * still the clone of a `<template>`, which belongs to the template's inert
 * document until it is attached. A `<video>` in that markup is therefore
 * created in the inert document and adopted into the page.
 *
 * WebKit (macOS 27.2) does not reliably draw a video that got into the page
 * that way. In the desktop app it plays with a blank picture, controls and
 * time advancing, once `requestVideoFrameCallback` has been called on it,
 * which the first-frame preview (videoPreview.ts) does for every note video;
 * when the call is made, or cancelling it, changes nothing. In a bare
 * WKWebView it is blank without the call. A copy of the same element made by
 * the page's document draws, with the same markup, source and preview.
 *
 * So wherever rendered markdown goes in with `{@html}`, each video is
 * replaced by such a copy before it has loaded anything:
 *
 *     <div class="markdown-content" {@attach pageVideos(html)}>{@html html}</div>
 *
 * This belongs to the render site, not to the document-wide watch in
 * videoPreview.ts: the editor builds its video with `document.createElement`
 * (wysiwygMedia.ts), and its node view holds on to that element.
 *
 * Only a capture of the real window shows the difference. jsdom has no media
 * pipeline, and `WKWebView.takeSnapshot` draws a picture either way.
 */
import type { Attachment } from 'svelte/attachments';

/** A video in its `figure`, as mediaRendering.ts writes it. */
const NOTE_VIDEO = '.markdown-media-video video';

/** The copies, which a later pass leaves alone: one may be playing by then. */
const madeByPage = new WeakSet<HTMLVideoElement>();

/**
 * Replace every note video under `root` with a copy made by the document
 * `root` is in. Once per video: one that is already a copy stays the element
 * it is, with whatever the reader has done to it.
 *
 * Only a video in its `figure` (mediaRendering.ts) is replaced, so the nodes
 * swapped are never the first or last of the markup, which Svelte keeps hold
 * of. Nothing is done under a root owned by a document with no window, such
 * as a template's: a copy made there would be one more element to adopt.
 * Which document owns the root is the test, not whether the root is attached:
 * a detached root that the page's document owns is remade like any other, and
 * nothing is reported. The attachment runs once per render and does not come
 * back, so a video left in a windowless document is reported: it would play
 * blank with nothing else to say why.
 */
export function remakeVideosInPage(root: Element): void {
  const page = root.ownerDocument;
  if (!page.defaultView) {
    if (root.querySelector(NOTE_VIDEO)) {
      console.warn(
        "[pageVideos] A note video was rendered in a document with no window, such as a template's, and was not remade; WebKit may play it with a blank picture"
      );
    }
    return;
  }
  for (const video of root.querySelectorAll<HTMLVideoElement>(NOTE_VIDEO)) {
    if (madeByPage.has(video)) continue;
    const copy = page.importNode(video, true);
    madeByPage.add(copy);
    video.replaceWith(copy);
    // The element left behind would go on loading its source for as long as
    // it lives; without one it holds no connection.
    video.removeAttribute('src');
    video.load();
  }
}

/**
 * For the element `{@html html}` fills. Svelte runs an attachment again when
 * it is handed a different function, so each render of the markup gets its
 * own, and that runs once the new nodes are in.
 */
export function pageVideos(_html: string | null): Attachment<Element> {
  return (node) => remakeVideosInPage(node);
}
