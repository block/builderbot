import { expect, it, vi } from 'vitest';
vi.mock('../../transport', () => ({ isTauri: true }));
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn(() => 'staged-media://localhost/file'),
}));
import { convertFileSrc } from '@tauri-apps/api/core';
import { resolveStagedMediaUrl } from './mediaUrl';
it('delegates platform-specific protocol URLs to Tauri', () => {
  expect(resolveStagedMediaUrl('id', 'png')).toBe('staged-media://localhost/file');
  expect(convertFileSrc).toHaveBeenCalledWith('id.png', 'staged-media');
});
