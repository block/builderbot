import { getToolImage, readImageFile } from '../../commands';
import { isTauri } from '../../transport';
import type { ToolCallImage } from './toolCallImages';

/**
 * Where `ToolResultImage` gets its `src` from.
 *
 * - `inline` payloads are already a data URL.
 * - `ref` images on the web resolve to `/api/tool-images/<ref>` and are handed
 *   straight to `<img loading="lazy">`, so the browser decides when (and
 *   whether) to download them. Under Tauri there is no HTTP server, so the
 *   bytes come back through the `get_tool_image` command instead.
 * - `file` images are read from disk through `read_image_file`.
 *
 * `immediateToolImageSrc` is the synchronous half; when it returns `null`
 * the component waits until the image is near the viewport and then calls
 * `fetchToolImageSrc`.
 */
export function immediateToolImageSrc(image: ToolCallImage): string | null {
  if (image.kind === 'inline') return image.dataUrl;
  if (image.kind === 'ref' && !isTauri) return toolImageUrl(image.ref.ref);
  return null;
}

export function fetchToolImageSrc(image: ToolCallImage): Promise<string> {
  switch (image.kind) {
    case 'inline':
      return Promise.resolve(image.dataUrl);
    case 'ref':
      return isTauri
        ? getToolImage(image.ref.messageId, image.ref.field, image.ref.index)
        : Promise.resolve(toolImageUrl(image.ref.ref));
    case 'file':
      return readImageFile(image.path);
  }
}

export function toolImageUrl(ref: string): string {
  return `/api/tool-images/${ref}`;
}
