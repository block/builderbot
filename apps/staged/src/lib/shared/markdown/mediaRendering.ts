import type { Parser, Tokens } from 'marked';
import type { ImageDimensions } from '../../types';
import { parseStagedMediaReference, resolveStagedMediaUrl } from './mediaUrl';

/** Stored size and density of an embedded image, when the caller has it. */
export type MediaDimensionsLookup = (id: string) => ImageDimensions | null | undefined;

function escapeHtml(text: string): string {
  return text
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

const EXPAND_BUTTON =
  '<button type="button" class="markdown-media-expand" aria-label="Open full screen" title="Open full screen">' +
  '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5"/></svg>' +
  '</button>';

/**
 * Only canonical IDs and supported extensions can bypass the sanitizer.
 *
 * `token.text` is the alt source as typed (`a\_b`, `**Before**`); like
 * marked's own image renderer, flatten the inline tokens to plain text first.
 * The flattened text is unescaped, so escaping still happens afterwards.
 */
export function renderMarkdownMedia(
  token: Tokens.Image,
  parser?: Parser,
  dimensionsFor?: MediaDimensionsLookup
): string | null {
  const media = parseStagedMediaReference(token.href);
  if (!media) return null;
  const { id, ext, kind } = media;
  const text =
    parser && token.tokens ? parser.parseInline(token.tokens, parser.textRenderer) : token.text;
  const alt = escapeHtml(text);
  const title = token.title ? ` title="${escapeHtml(token.title)}"` : '';
  const src = escapeHtml(resolveStagedMediaUrl(id, ext));
  // Videos get no expand button: the native controls already offer full
  // screen, and an overlay would sit over them.
  return [
    `<figure class="markdown-media markdown-media-${kind}" data-media-id="${id}" data-media-ext="${ext}" data-media-alt="${alt}">`,
    kind === 'video'
      ? `<video src="${src}" controls preload="metadata" playsinline aria-label="${alt || 'Video'}"${title}></video>`
      : `<img src="${src}"${imageSizeAttributes(src, dimensionsFor?.(id))} alt="${alt}" loading="lazy"${title}>` +
        EXPAND_BUTTON,
    alt ? `<figcaption>${alt}</figcaption>` : '',
    '</figure>',
  ].join('');
}

/**
 * Lay a stored image out at its logical size.
 *
 * `width`/`height` are the pixel size over the density the file's own metadata
 * declares, so a 1320x960 Retina capture takes 660x480 (and reserves that box
 * before it loads), while a 1x screenshot, a photo, or a rasterized PDF page
 * keeps its pixel size. The `Nx` descriptor makes the browser agree about the
 * intrinsic size on any screen, and is only emitted when it says something.
 * The custom properties repeat the logical width and the aspect for
 * mediaStyles.css, which applies the height cap through `width` so a capped
 * portrait image shrinks on both axes (an attribute width alone would make
 * `max-height` letterbox it). Nothing is emitted for an image with no stored
 * metadata, which then falls back to the CSS caps alone.
 */
function imageSizeAttributes(src: string, dimensions: ImageDimensions | null | undefined): string {
  if (!dimensions) return '';
  const { width, height, density } = dimensions;
  if (!(width > 0 && height > 0 && density >= 1)) return '';
  const logicalWidth = Math.round(width / density);
  const logicalHeight = Math.round(height / density);
  const srcset = density > 1 ? ` srcset="${src} ${density}x"` : '';
  return (
    `${srcset} width="${logicalWidth}" height="${logicalHeight}"` +
    ` style="--media-width: ${logicalWidth}px; --media-aspect: ${(width / height).toFixed(4)}"`
  );
}
