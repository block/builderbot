import { getToolCallBody } from '../../commands';
import { isTauri } from '../../transport';
import type { ToolCallBody } from '../../types';

/**
 * Where `ToolCallCard` gets a deferred body from (see `toolCallBodies.ts`).
 *
 * On the web the bodies are a plain `GET /api/tool-bodies/<messageId>` —
 * JSON, so it is compressed like the transcript, and long-lived cacheable
 * since only settled rows are deferred. Under Tauri there is no HTTP server,
 * so they come back through the `get_tool_call_body` command.
 *
 * Bodies are fetched once per row and kept here, so collapsing and
 * re-expanding a card — or re-mounting it after paging — never asks again.
 * A failed fetch is dropped from the cache so a retry really retries.
 */

const MAX_CACHED_BODIES = 200;

const bodies = new Map<number, Promise<ToolCallBody>>();

export function toolBodyUrl(messageId: number): string {
  return `/api/tool-bodies/${messageId}`;
}

export function fetchToolCallBody(messageId: number): Promise<ToolCallBody> {
  if (isTauri) return getToolCallBody(messageId);
  return fetch(toolBodyUrl(messageId)).then(async (response) => {
    if (!response.ok) {
      throw new Error(`Tool call body ${messageId}: HTTP ${response.status}`);
    }
    return (await response.json()) as ToolCallBody;
  });
}

export function loadToolCallBody(messageId: number): Promise<ToolCallBody> {
  const cached = bodies.get(messageId);
  if (cached) return cached;
  const request: Promise<ToolCallBody> = fetchToolCallBody(messageId).catch((error: unknown) => {
    if (bodies.get(messageId) === request) bodies.delete(messageId);
    throw error;
  });
  bodies.set(messageId, request);
  if (bodies.size > MAX_CACHED_BODIES) {
    // Insertion order is oldest first; the cap only bounds memory on very
    // long sessions, so a dropped entry is simply fetched again.
    const oldest = bodies.keys().next().value;
    if (oldest !== undefined) bodies.delete(oldest);
  }
  return request;
}

export async function loadToolCallBodies(
  messageIds: readonly number[]
): Promise<Map<number, ToolCallBody>> {
  const loaded = await Promise.all(messageIds.map((id) => loadToolCallBody(id)));
  return new Map(loaded.map((body, index) => [messageIds[index], body]));
}

/** Test seam. */
export function resetToolCallBodyCache(): void {
  bodies.clear();
}
