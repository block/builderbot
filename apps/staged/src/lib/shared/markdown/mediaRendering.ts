import type { Tokens } from 'marked';
import { parseStagedMediaReference, resolveStagedMediaUrl } from './mediaUrl';

function escapeHtml(text: string): string {
  return text
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

/** Only canonical IDs and supported extensions can bypass the sanitizer. */
export function renderMarkdownMedia(token: Tokens.Image): string | null {
  const media = parseStagedMediaReference(token.href);
  if (!media) return null;
  const { id, ext, kind } = media;
  const alt = escapeHtml(token.text);
  const title = token.title ? ` title="${escapeHtml(token.title)}"` : '';
  const src = escapeHtml(resolveStagedMediaUrl(id, ext));
  return [
    `<figure class="markdown-media markdown-media-${kind}" data-media-id="${id}" data-media-ext="${ext}" data-media-alt="${alt}">`,
    kind === 'video'
      ? `<video src="${src}" controls preload="metadata" playsinline aria-label="${alt || 'Video'}"${title}></video>`
      : `<img src="${src}" alt="${alt}" loading="lazy"${title}>`,
    '<button type="button" class="markdown-media-expand" aria-label="Open full screen" title="Open full screen">',
    '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5"/></svg>',
    '</button>',
    alt ? `<figcaption>${alt}</figcaption>` : '',
    '</figure>',
  ].join('');
}
