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
 * the plain `<img>` the preset draws. A selected video takes Space and Enter
 * for playback (`stagedMediaKeys`) rather than being typed over by them.
 */
import { imageSchema } from '@milkdown/kit/preset/commonmark';
import { DOMSerializer, type Node as ProseNode } from '@milkdown/kit/prose/model';
import { NodeSelection, Plugin, PluginKey } from '@milkdown/kit/prose/state';
import type { EditorView, NodeView } from '@milkdown/kit/prose/view';
import { $prose, $view } from '@milkdown/kit/utils';

import { getMediaDimensions } from '../../api/commands';
import { mediaImageLayout } from '../../shared/markdown/mediaRendering';
import '../../shared/markdown/mediaStyles.css';
import {
  parseStagedMediaReference,
  resolveStagedMediaUrl,
  type MediaKind,
} from '../../shared/markdown/mediaUrl';
import type { ImageDimensions } from '../../types';

export const stagedMediaView = $view(imageSchema.node, () => (node, view, getPos): NodeView => {
  const media = parseStagedMediaReference(node.attrs.src);
  if (!media) {
    const dom = DOMSerializer.fromSchema(view.state.schema).serializeNode(node) as HTMLElement;
    return { dom };
  }
  const dom = mediaFigure(node, media);
  const video = dom.querySelector('video');
  if (video) selectVideoLikeAnImage(video, view, getPos);
  return {
    dom,
    // Same node, same figure; anything else is redrawn from scratch.
    update: (updated) => updated.sameMarkup(node),
    // The figure is not editable and the browser owns the video's controls:
    // ProseMirror must not turn clicks, keys and focus inside them into a drag
    // or a preventDefault (`selectVideoLikeAnImage` selects it instead).
    // Everything else (clicking an image, for one) stays with ProseMirror, so
    // the usual select-then-delete applies.
    stopEvent: (event) => video?.contains(event.target as Element) ?? false,
    // No `ignoreMutation`: for a view without a contentDOM, ProseMirror's
    // default already ignores every mutation but a selection change. So the
    // dimension lookup below and the video's own state changes are not taken
    // for edits, and drag-selecting a caption keeps the selection in step.
  };
});

/**
 * Space and Enter play or pause a selected video instead of reaching
 * ProseMirror, which would type them over the node (Enter splits the block
 * there). This covers focus in the editor; the video's own keydown handler
 * (`selectVideoLikeAnImage`) covers focus on the video, whose events this
 * never sees. It is a DOM-event handler rather than a keymap so it runs ahead
 * of every keymap, the preset's Enter included. Other keys are left alone:
 * typing replaces a selected video as it does a selected image.
 */
export const stagedMediaKeys = $prose(
  () =>
    new Plugin({
      key: new PluginKey('WRITTEN_NOTE_MEDIA_KEYS'),
      props: {
        handleDOMEvents: {
          keydown: (view, event) => {
            if (!isPlaybackKey(event)) return false;
            const video = selectedVideo(view);
            if (!video) return false;
            event.preventDefault();
            togglePlayback(video);
            return true;
          },
        },
      },
    })
);

/** The video drawn for the node-selected media, if that is what is selected. */
function selectedVideo(view: EditorView): HTMLVideoElement | null {
  const { selection } = view.state;
  if (!(selection instanceof NodeSelection) || selection.node.type.name !== 'image') return null;
  const dom = view.nodeDOM(selection.from);
  return dom instanceof HTMLElement ? dom.querySelector('video') : null;
}

function isPlaybackKey(event: KeyboardEvent): boolean {
  return (
    (event.key === ' ' || event.key === 'Enter') &&
    !event.ctrlKey &&
    !event.metaKey &&
    !event.altKey &&
    !event.isComposing
  );
}

function togglePlayback(video: HTMLVideoElement) {
  if (video.paused) {
    // Rejects when the source can't play; the controls show that already.
    video.play().catch(() => {});
  } else {
    video.pause();
  }
}

/**
 * Give a video the click-then-delete an image has. Its events are kept from
 * ProseMirror (see `stopEvent`), and a caption-less video's figure has no
 * other surface to click, so the node view selects it itself. The selection
 * follows the click rather than the press, as ProseMirror's own does, so the
 * browser's handling of the press (focusing the video, scrubbing) is done by
 * then; nothing is prevented, so the controls still work. Native controls are
 * not in the light DOM, so a click on them selects the video too.
 *
 * A click that started playback leaves focus where the browser put it.
 * Otherwise the editor takes it, unless the browser kept it on the video for
 * its controls. ProseMirror never sees a key there, so Backspace and Delete on
 * the selected video remove it, and Space and Enter play or pause it, as
 * `stagedMediaKeys` does in the editor.
 */
function selectVideoLikeAnImage(
  video: HTMLVideoElement,
  view: EditorView,
  getPos: () => number | undefined
) {
  const isSelected = () => {
    const { selection } = view.state;
    return selection instanceof NodeSelection && selection.from === getPos();
  };
  // Read before the click reaches the controls in the video's shadow tree.
  // WebKit's (the desktop build's) handle it there, so a click that started
  // playback reads as one below. Where the toggle is the click's default
  // action instead, it hasn't happened yet below, and the editor is focused
  // as for any other click; Space then pauses through `stagedMediaKeys`.
  let pausedBeforeClick = video.paused;
  video.addEventListener(
    'click',
    () => {
      pausedBeforeClick = video.paused;
    },
    { capture: true }
  );
  video.addEventListener('click', (event) => {
    const pos = getPos();
    if (event.button !== 0 || pos === undefined || !view.editable) return;
    if (!isSelected()) {
      view.dispatch(view.state.tr.setSelection(NodeSelection.create(view.state.doc, pos)));
    }
    const startedPlayback = pausedBeforeClick && !video.paused;
    if (!startedPlayback && document.activeElement !== video) view.focus();
  });
  video.addEventListener('keydown', (event) => {
    if (!view.editable || !isSelected()) return;
    if (isPlaybackKey(event)) {
      event.preventDefault();
      togglePlayback(video);
    } else if (event.key === 'Backspace' || event.key === 'Delete') {
      event.preventDefault();
      view.dispatch(view.state.tr.deleteSelection().scrollIntoView());
      view.focus();
    }
  });
}

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
