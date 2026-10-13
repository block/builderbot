import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ToolCallImage } from './toolCallImages';

const transport = vi.hoisted(() => ({ isTauri: false }));
const commands = vi.hoisted(() => ({
  getToolImage: vi.fn(),
  readImageFile: vi.fn(),
}));

vi.mock('../../transport', () => transport);
vi.mock('../../commands', () => commands);

import { fetchToolImageSrc, immediateToolImageSrc } from './toolImageSource';

const ref: ToolCallImage = {
  kind: 'ref',
  ref: { ref: '42/output/1', messageId: 42, field: 'output', index: 1 },
  mimeType: 'image/png',
  label: 'Tool result image',
};
const inline: ToolCallImage = { kind: 'inline', dataUrl: 'data:image/png;base64,AA==', label: '' };
const file: ToolCallImage = { kind: 'file', path: '/tmp/shot.png', label: 'shot.png' };

describe('tool image sources', () => {
  beforeEach(() => {
    transport.isTauri = false;
    commands.getToolImage.mockReset();
    commands.readImageFile.mockReset();
  });

  it('hands the browser the lazy /api/tool-images URL for a ref', async () => {
    expect(immediateToolImageSrc(ref)).toBe('/api/tool-images/42/output/1');
    await expect(fetchToolImageSrc(ref)).resolves.toBe('/api/tool-images/42/output/1');
    expect(commands.getToolImage).not.toHaveBeenCalled();
  });

  it('fetches a ref through the get_tool_image command under Tauri', async () => {
    transport.isTauri = true;
    commands.getToolImage.mockResolvedValue('data:image/png;base64,BB==');

    expect(immediateToolImageSrc(ref)).toBeNull();
    await expect(fetchToolImageSrc(ref)).resolves.toBe('data:image/png;base64,BB==');
    expect(commands.getToolImage).toHaveBeenCalledWith(42, 'output', 1);
  });

  it('uses inline payloads as-is and reads file images on demand', async () => {
    commands.readImageFile.mockResolvedValue('data:image/png;base64,CC==');

    expect(immediateToolImageSrc(inline)).toBe(inline.dataUrl);
    expect(immediateToolImageSrc(file)).toBeNull();
    await expect(fetchToolImageSrc(file)).resolves.toBe('data:image/png;base64,CC==');
    expect(commands.readImageFile).toHaveBeenCalledWith('/tmp/shot.png');
  });
});
