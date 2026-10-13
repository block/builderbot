import { formatJson } from './acpTranscript';
import { stripCodeFences } from './sessionModalHelpers';

export type ToolCallImage =
  | { kind: 'inline'; dataUrl: string; label: string }
  /** A server-projected image: the bytes live behind `/api/tool-images/<ref>`. */
  | { kind: 'ref'; ref: ToolImageRef; mimeType: string; label: string }
  | { kind: 'file'; path: string; label: string };

/**
 * `<message id>/<output|content>/<block index>`, as the store writes it in
 * place of a base64 image block (`store/tool_images.rs`). Parsed into its
 * parts here so the component never has to split the string again.
 */
export interface ToolImageRef {
  ref: string;
  messageId: number;
  field: 'output' | 'content';
  index: number;
}

const IMAGE_MIME_TYPE = /^image\/[a-z0-9.+-]+$/i;

export function parseToolImageRef(value: unknown): ToolImageRef | null {
  if (typeof value !== 'string') return null;
  const match = /^(\d+)\/(output|content)\/(\d+)$/.exec(value);
  if (!match) return null;
  return {
    ref: value,
    messageId: Number(match[1]),
    field: match[2] as ToolImageRef['field'],
    index: Number(match[3]),
  };
}

export function toolCallImageContent(value: unknown): { images: ToolCallImage[]; text: string } {
  const blocks = Array.isArray(value)
    ? value
    : isRecord(value) && Array.isArray(value.content)
      ? value.content
      : [];
  const images: ToolCallImage[] = [];
  const text: string[] = [];
  for (const entry of blocks) {
    const block = isRecord(entry) && entry.type === 'content' ? entry.content : entry;
    if (!isRecord(block)) continue;
    if (block.type === 'text' && typeof block.text === 'string') {
      text.push(block.text);
    } else if (block.type === 'image') {
      // ACP/MCP embed data directly; Claude's raw result nests it in source.
      const source = isRecord(block.source) ? block.source : block;
      const mimeType = source.mimeType ?? source.media_type;
      const ref = parseToolImageRef(block.ref);
      if (ref && typeof mimeType === 'string' && IMAGE_MIME_TYPE.test(mimeType)) {
        // The store's projection of a base64 block (fresh payloads).
        images.push({ kind: 'ref', ref, mimeType, label: 'Tool result image' });
      } else if (
        // Inline base64: payloads cached in IndexedDB before the projection
        // existed, and any producer that still embeds the bytes.
        typeof source.data === 'string' &&
        source.data.length > 0 &&
        typeof mimeType === 'string' &&
        IMAGE_MIME_TYPE.test(mimeType)
      ) {
        images.push({
          kind: 'inline',
          dataUrl: `data:${mimeType};base64,${source.data}`,
          label: 'Tool result image',
        });
      }
    } else if (block.type === 'resource_link' && typeof block.uri === 'string') {
      const path = localImagePath(block.uri);
      if (path) {
        images.push({
          kind: 'file',
          path,
          label: typeof block.name === 'string' && block.name ? block.name : path,
        });
      }
    }
  }
  return { images, text: text.join('\n').trim() };
}

export function toolCallOutputText(value: unknown): string {
  const content = toolCallImageContent(value);
  if (content.images.length === 0) return formatJson(value);

  // Image blocks replace only their payloads, not the result's sibling text fields.
  const parts = isRecord(value)
    ? ['output', 'text', 'body', 'response', 'result'].map((key) => toolCallOutputText(value[key]))
    : [];
  parts.push(stripCodeFences(content.text));
  return [...new Set(parts.filter(Boolean))].join('\n');
}

function localImagePath(uri: string): string | null {
  let path = uri;
  if (uri.startsWith('file:')) {
    try {
      const url = new URL(uri);
      if (url.hostname && url.hostname !== 'localhost') return null;
      path = decodeURIComponent(url.pathname);
    } catch {
      return null;
    }
  }
  // Codex's View Image results use absolute paths without a MIME type.
  return path.startsWith('/') && /\.(png|jpe?g|gif|webp)$/i.test(path) ? path : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
