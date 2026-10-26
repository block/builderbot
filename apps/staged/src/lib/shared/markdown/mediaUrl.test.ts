import { expect, it, vi } from 'vitest';
vi.mock('../../transport', () => ({ isTauri: true }));
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn(() => 'staged-media://localhost/file'),
}));
import { convertFileSrc } from '@tauri-apps/api/core';
import {
  collectStagedMediaImageIds,
  parseStagedMediaReference,
  resolvePdfAttachmentUrl,
  resolveStagedMediaUrl,
} from './mediaUrl';
it('delegates platform-specific protocol URLs to Tauri', () => {
  expect(resolveStagedMediaUrl('id', 'png')).toBe('staged-media://localhost/file');
  expect(convertFileSrc).toHaveBeenCalledWith('id.png', 'staged-media');
});

it('resolves PDF attachments by id without admitting them as note embeds', () => {
  expect(resolvePdfAttachmentUrl('id')).toBe('staged-media://localhost/file');
  expect(convertFileSrc).toHaveBeenCalledWith('id.pdf', 'staged-media');
  expect(
    parseStagedMediaReference('staged-media://0f3b9d2a-1c4e-4a5b-8d6e-7f8091a2b3c4.pdf')
  ).toBeNull();
});

it('collects each embedded image id once, skipping videos and malformed references', () => {
  const a = '0f3b9d2a-1c4e-4a5b-8d6e-7f8091a2b3c4';
  const b = '1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d';
  const c = '2b3c4d5e-6f7a-4b8c-9d0e-1f2a3b4c5d6e';
  expect(
    collectStagedMediaImageIds(
      `![a](staged-media://${a}.png) ![b](staged-media://${b}.jpeg "t")\n` +
        `![again](staged-media://${a}.png) ![clip](staged-media://${c}.mp4)\n` +
        `![bad](staged-media://nope.png) ![svg](staged-media://${c}.svg)`
    )
  ).toEqual([a, b]);
  expect(collectStagedMediaImageIds('no media here')).toEqual([]);
});
