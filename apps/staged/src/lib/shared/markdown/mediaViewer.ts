import { parseStagedMediaReference } from './mediaUrl';

export interface MarkdownMedia {
  id: string;
  ext: string;
  alt: string;
}

/**
 * Only images open in the viewer; videos have no trigger, so clicks on them
 * reach the native controls untouched.
 */
export function getMarkdownMediaFromEvent(target: EventTarget | null): MarkdownMedia | null {
  if (!(target instanceof Element)) return null;
  const trigger = target.closest('.markdown-media-image :is(img, .markdown-media-expand)');
  return trigger && getMarkdownMedia(trigger);
}

/** The inline image a click on it, or on its expand button, opens the viewer from. */
export function getMarkdownMediaImageFromEvent(
  target: EventTarget | null
): HTMLImageElement | null {
  if (!(target instanceof Element)) return null;
  return target.closest('.markdown-media-image')?.querySelector('img') ?? null;
}

/** The media of the figure `element` is in, when that is an image. */
export function getMarkdownMedia(element: Element): MarkdownMedia | null {
  const figure = element.closest('.markdown-media');
  if (!figure) return null;
  const media = parseStagedMediaReference(
    `staged-media://${figure.getAttribute('data-media-id')}.${figure.getAttribute('data-media-ext')}`
  );
  if (!media || media.kind !== 'image') return null;
  return { id: media.id, ext: media.ext, alt: figure.getAttribute('data-media-alt') ?? '' };
}
