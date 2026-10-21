import { beforeEach, describe, expect, it, vi } from 'vitest';

const { open } = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open }));

import { IMAGE_PICKER_FILTER, pickImagePaths } from './branchImagePicker';
import { isImageAttachmentFile, isImageFile } from './branchCardHelpers';

describe('pickImagePaths', () => {
  beforeEach(() => {
    open.mockReset();
  });

  it('opens a multi-select file panel filtered to images', async () => {
    open.mockResolvedValue([]);
    await pickImagePaths();
    expect(open).toHaveBeenCalledWith({
      title: 'Add image',
      multiple: true,
      directory: false,
      filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp'] }],
    });
  });

  it('returns the picked paths', async () => {
    open.mockResolvedValue(['/Users/me/shot.png', '/Users/me/photo.jpg']);
    expect(await pickImagePaths()).toEqual(['/Users/me/shot.png', '/Users/me/photo.jpg']);
  });

  it('returns no paths when the user cancels', async () => {
    open.mockResolvedValue(null);
    expect(await pickImagePaths()).toEqual([]);
  });

  it('surfaces a panel failure to the caller', async () => {
    open.mockRejectedValue(new Error('dialog.open not allowed'));
    await expect(pickImagePaths()).rejects.toThrow('dialog.open not allowed');
  });
});

describe('IMAGE_PICKER_FILTER', () => {
  it('offers exactly the image files a branch-card drop stores', () => {
    for (const ext of IMAGE_PICKER_FILTER.extensions) {
      expect(isImageFile(`photo.${ext}`)).toBe(true);
      expect(isImageAttachmentFile(`photo.${ext}`)).toBe(true);
    }
    expect(IMAGE_PICKER_FILTER.extensions).not.toContain('pdf');
  });
});
