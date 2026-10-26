import { convertFileSrc } from '@tauri-apps/api/core';
import { isTauri } from '../../transport';

// One pattern, anchored for parsing a single href and unanchored (global) for
// finding every reference in a markdown text; both capture (id, ext).
const mediaReferencePattern =
  /staged-media:\/\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.(png|jpg|jpeg|gif|webp|mp4|webm|mov)/;
const mediaReference = new RegExp(`^${mediaReferencePattern.source}$`);
const mediaImageOccurrence = new RegExp(mediaReferencePattern.source, 'g');

export type MediaKind = 'image' | 'video';

export function parseStagedMediaReference(href: string) {
  const match = mediaReference.exec(href);
  if (!match) return null;
  const [, id, ext] = match;
  const kind: MediaKind = ['mp4', 'webm', 'mov'].includes(ext) ? 'video' : 'image';
  return { id, ext, kind };
}

/** Ids of the images (not videos) a markdown text embeds, each once, in order. */
export function collectStagedMediaImageIds(markdown: string): string[] {
  const ids = new Set<string>();
  for (const [, id, ext] of markdown.matchAll(mediaImageOccurrence)) {
    if (parseStagedMediaReference(`staged-media://${id}.${ext}`)?.kind === 'image') ids.add(id);
  }
  return [...ids];
}

export function resolveStagedMediaUrl(id: string, ext: string): string {
  return isTauri ? convertFileSrc(`${id}.${ext}`, 'staged-media') : `/api/media/${id}.${ext}`;
}

/**
 * Served URL of a PDF chat attachment, for the viewer's `<iframe>`. Looked up
 * by id like note media (the same protocol and `/api/media` route, Range and
 * all), but deliberately not via `parseStagedMediaReference`: PDFs stay out
 * of note embeds.
 */
export function resolvePdfAttachmentUrl(id: string): string {
  return resolveStagedMediaUrl(id, 'pdf');
}
