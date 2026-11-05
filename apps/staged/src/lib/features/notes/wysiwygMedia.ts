/**
 * wysiwygMedia.ts — draw stored note media inside the written-note editor
 *
 * A written note can carry `staged-media://<id>.<ext>` references: a copy of
 * an agent's note keeps the media the transfer copied, and an agent's media
 * can be pasted in. Milkdown's stock image node writes that URL straight into
 * `<img src>`, which nothing can load, and draws a video as an image. This
 * node view resolves the reference the way the viewer does
 * (`resolveStagedMediaUrl`) and emits the viewer's `<figure>` markup
 * (`mediaRendering.ts`), so `mediaStyles.css` sizes it the same. Images are
 * laid out at their logical size once their stored dimensions arrive, as in
 * NoteModal; until then (and for videos) the CSS caps alone apply.
 *
 * Only the DOM is replaced. The node and its `src` are left alone, so the
 * document still serializes to the stored reference, and selecting, deleting
 * and undoing work as for any inline atom. Images with any other `src` keep
 * the plain `<img>` the preset draws.
 */
import { imageSchema } from '@milkdown/kit/preset/commonmark';
import { DOMSerializer, type Node as ProseNode } from '@milkdown/kit/prose/model';
import type { NodeView } from '@milkdown/kit/prose/view';
import { $view } from '@milkdown/kit/utils';

import { getMediaDimensions } from '../../api/commands';
import { mediaImageLayout } from '../../shared/markdown/mediaRendering';
import '../../shared/markdown/mediaStyles.css';
import {
  parseStagedMediaReference,
  resolveStagedMediaUrl,
  type MediaKind,
} from '../../shared/markdown/mediaUrl';
import type { ImageDimensions } from '../../types';

export const stagedMediaView = $view(imageSchema.node, () => (node, view): NodeView => {
  const media = parseStagedMediaReference(node.attrs.src);
  if (!media) {
    const dom = DOMSerializer.fromSchema(view.state.schema).serializeNode(node) as HTMLElement;
    return { dom };
  }
  const dom = mediaFigure(node, media);
  return {
    dom,
    // Same node, same figure; anything else is redrawn from scratch.
    update: (updated) => updated.sameMarkup(node),
    // The figure is not editable and the browser owns the video's controls:
    // clicks, keys and focus inside them must not become a node selection or
    // a drag. Everything else (clicking an image, for one) stays with
    // ProseMirror, so the usual select-then-delete applies.
    stopEvent: (event) => dom.querySelector('video')?.contains(event.target as Element) ?? false,
    // Nothing inside is document content; the dimension lookup below and the
    // video's own state changes are not edits.
    ignoreMutation: () => true,
  };
});

/** The viewer's figure for one stored reference (see `renderMarkdownMedia`). */
function mediaFigure(
  node: ProseNode,
  { id, ext, kind }: { id: string; ext: string; kind: MediaKind }
): HTMLElement {
  const alt = String(node.attrs.alt ?? '');
  const title = String(node.attrs.title ?? '');
  const src = resolveStagedMediaUrl(id, ext);

  const figure = document.createElement('figure');
  figure.className = `markdown-media markdown-media-${kind}`;
  figure.setAttribute('contenteditable', 'false');
  figure.dataset.mediaId = id;
  figure.dataset.mediaExt = ext;
  figure.dataset.mediaAlt = alt;

  if (kind === 'video') {
    const video = document.createElement('video');
    video.setAttribute('src', src);
    video.setAttribute('controls', '');
    video.setAttribute('preload', 'metadata');
    video.setAttribute('playsinline', '');
    video.setAttribute('aria-label', alt || 'Video');
    if (title) video.setAttribute('title', title);
    figure.append(video);
  } else {
    const img = document.createElement('img');
    img.setAttribute('src', src);
    img.setAttribute('alt', alt);
    img.setAttribute('loading', 'lazy');
    if (title) img.setAttribute('title', title);
    figure.append(img);
    void dimensionsOf(id).then((dimensions) => sizeImage(img, src, dimensions));
  }

  if (alt) {
    const caption = document.createElement('figcaption');
    caption.textContent = alt;
    figure.append(caption);
  }
  return figure;
}

/** Apply `imageSizeAttributes` to a live image once its dimensions are known. */
function sizeImage(img: HTMLImageElement, src: string, dimensions: ImageDimensions | null) {
  const layout = mediaImageLayout(dimensions);
  if (!layout) return;
  if (layout.density > 1) img.setAttribute('srcset', `${src} ${layout.density}x`);
  img.setAttribute('width', String(layout.width));
  img.setAttribute('height', String(layout.height));
  img.style.setProperty('--media-width', `${layout.width}px`);
  img.style.setProperty('--media-aspect', layout.aspect);
}

/*
 * Stored dimensions, looked up once per image id for the life of the app (an
 * id names immutable bytes). ProseMirror builds a document's node views in
 * one go, so the ids asked for in the same tick share one backend call; a miss
 * is remembered as null, a failed call is not, so it can be retried later.
 */
const knownDimensions = new Map<string, Promise<ImageDimensions | null>>();
let pendingBatch: { ids: string[]; found: Promise<Record<string, ImageDimensions>> } | null = null;

function dimensionsOf(id: string): Promise<ImageDimensions | null> {
  const known = knownDimensions.get(id);
  if (known) return known;
  if (!pendingBatch) {
    const ids: string[] = [];
    pendingBatch = {
      ids,
      found: Promise.resolve().then(() => {
        pendingBatch = null;
        return getMediaDimensions(ids);
      }),
    };
  }
  pendingBatch.ids.push(id);
  const lookup = pendingBatch.found.then(
    (found) => found[id] ?? null,
    (error) => {
      console.warn('Failed to load note image dimensions', error);
      knownDimensions.delete(id);
      return null;
    }
  );
  knownDimensions.set(id, lookup);
  return lookup;
}
