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
 * The desktop app's loopback video stream, `http://127.0.0.1:<port>/<secret>`
 * (src-tauri `note_media/stream.rs`). The app sets the global in each window
 * before any script runs, and sets nothing when its listener could not start.
 */
const videoStreamBase = /^http:\/\/127\.0\.0\.1:\d{1,5}\/[0-9a-f]{64}$/;
const videoStreamUrl =
  /^http:\/\/127\.0\.0\.1:\d{1,5}\/[0-9a-f]{64}\/([0-9a-f-]{36})\.(mp4|webm|mov)(#.*)?$/;

/**
 * Served URL of a note video. On desktop that is the loopback stream when
 * there is one: over the `staged-media` protocol WebKit fetches an MP4 one
 * sample at a time through the app main thread and takes a WebM whole into
 * memory, while over HTTP it streams with a few ranged requests. Everything
 * else (images, PDFs, web and mobile clients) is `resolveStagedMediaUrl`.
 */
export function resolveStagedVideoUrl(id: string, ext: string): string {
  const base = isTauri
    ? (globalThis as { __STAGED_MEDIA_STREAM__?: unknown }).__STAGED_MEDIA_STREAM__
    : null;
  return typeof base === 'string' && videoStreamBase.test(base)
    ? `${base}/${id}.${ext}`
    : resolveStagedMediaUrl(id, ext);
}

/**
 * For a video URL on the loopback stream, the same video (and fragment) on
 * the `staged-media` protocol; null for any other URL. A `<video>` whose
 * stream failed is retried there (`videoPreview.ts`).
 */
export function protocolUrlForStreamedVideo(src: string): string | null {
  const match = isTauri ? videoStreamUrl.exec(src) : null;
  if (!match) return null;
  const [, id, ext, fragment = ''] = match;
  return resolveStagedMediaUrl(id, ext) + fragment;
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
