import { convertFileSrc } from '@tauri-apps/api/core';
import { isTauri } from '../../transport';

const mediaReference =
  /^staged-media:\/\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.(png|jpg|jpeg|gif|webp|mp4|webm|mov)$/;

export type MediaKind = 'image' | 'video';

export function parseStagedMediaReference(href: string) {
  const match = mediaReference.exec(href);
  if (!match) return null;
  const [, id, ext] = match;
  const kind: MediaKind = ['mp4', 'webm', 'mov'].includes(ext) ? 'video' : 'image';
  return { id, ext, kind };
}

export function resolveStagedMediaUrl(id: string, ext: string): string {
  return isTauri ? convertFileSrc(`${id}.${ext}`, 'staged-media') : `/api/media/${id}.${ext}`;
}
