import { parseStagedMediaReference, type MediaKind } from './mediaUrl';

export interface MarkdownMedia {
  id: string;
  ext: string;
  kind: MediaKind;
  alt: string;
}

export function getMarkdownMediaFromEvent(target: EventTarget | null): MarkdownMedia | null {
  if (!(target instanceof Element)) return null;
  const trigger = target.closest('.markdown-media-expand, .markdown-media-image > img');
  if (!trigger) return null;
  const figure = trigger.closest('.markdown-media');
  if (!figure) return null;
  const media = parseStagedMediaReference(
    `staged-media://${figure.getAttribute('data-media-id')}.${figure.getAttribute('data-media-ext')}`
  );
  return media ? { ...media, alt: figure.getAttribute('data-media-alt') ?? '' } : null;
}
