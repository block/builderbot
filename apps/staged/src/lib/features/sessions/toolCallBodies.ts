import type { SessionMessage, ToolCallBody } from '../../types';
import type { RichToolItem } from './acpTranscript';

/**
 * Deferred tool bodies.
 *
 * The store ships the large input/output/content of a *settled* tool row as
 * a marker in place of the body (`store/tool_bodies.rs`):
 *
 *     {"$deferred": {"messageId": 42, "field": "output", "bytes": 18231,
 *                    "innerSessionId"?: "…", "pikchr"?: "…"}}
 *
 * A collapsed card needs nothing from a body beyond "there is one" — plus the
 * two pikchr facts the marker carries as hints — so the card fetches the row's
 * bodies through `toolBodySource.ts` only once it is expanded, then renders
 * the item with the markers swapped out ({@link hydrateToolItem}). Rows from
 * payloads cached before this projection existed, and in-flight rows, carry
 * their bodies inline and never produce markers.
 */

export const DEFERRED_TOOL_BODY_KEY = '$deferred';

export type ToolBodyField = 'input' | 'output' | 'content';

export interface DeferredToolBody {
  messageId: number;
  field: ToolBodyField;
  /** Serialized size of the body the marker stands in for. */
  bytes: number;
  /** `innerSessionIdFromValue` of the body: the diagram session a `generate_pikchr` result names. */
  innerSessionId: string | null;
  /** `rawInput.pikchr` of a `render_pikchr` call, drawn inline by the collapsed card. */
  pikchr: string | null;
}

const BODY_KEY_BY_FIELD: Record<ToolBodyField, keyof ToolCallBody> = {
  input: 'acpRawInput',
  output: 'acpRawOutput',
  content: 'acpContent',
};

export function deferredToolBody(value: unknown): DeferredToolBody | null {
  if (!isRecord(value)) return null;
  const marker = value[DEFERRED_TOOL_BODY_KEY];
  if (!isRecord(marker)) return null;
  const { messageId, field } = marker;
  if (typeof messageId !== 'number' || !Number.isInteger(messageId) || messageId < 0) return null;
  if (field !== 'input' && field !== 'output' && field !== 'content') return null;
  return {
    messageId,
    field,
    bytes: typeof marker.bytes === 'number' && Number.isFinite(marker.bytes) ? marker.bytes : 0,
    innerSessionId: nonEmptyString(marker.innerSessionId),
    pikchr: nonEmptyString(marker.pikchr),
  };
}

type ToolBodies = Pick<RichToolItem, 'rawInput' | 'rawOutput' | 'content'>;

/** Rows whose bodies the item's markers point at, deduplicated and ascending. */
export function deferredToolBodyIds(item: ToolBodies): number[] {
  const ids = new Set<number>();
  for (const value of [item.rawInput, item.rawOutput, item.content]) {
    const deferred = deferredToolBody(value);
    if (deferred) ids.add(deferred.messageId);
  }
  return [...ids].sort((a, b) => a - b);
}

/**
 * The item with each marker replaced by the matching field of its fetched
 * body. A marker whose body has not been fetched stays as it is; a fetched
 * body missing the field yields `null`, which the view model reads as "no
 * body". Returns `item` itself when nothing changed, so a card without
 * markers keeps its identity.
 */
export function hydrateToolItem<T extends ToolBodies>(
  item: T,
  bodies: ReadonlyMap<number, ToolCallBody>
): T {
  const rawInput = resolveBody(item.rawInput, bodies);
  const rawOutput = resolveBody(item.rawOutput, bodies);
  const content = resolveBody(item.content, bodies);
  if (rawInput === item.rawInput && rawOutput === item.rawOutput && content === item.content) {
    return item;
  }
  return { ...item, rawInput, rawOutput, content };
}

function resolveBody(value: unknown, bodies: ReadonlyMap<number, ToolCallBody>): unknown {
  const deferred = deferredToolBody(value);
  if (!deferred) return value;
  const body = bodies.get(deferred.messageId);
  if (!body) return value;
  return body[BODY_KEY_BY_FIELD[deferred.field]] ?? null;
}

const MESSAGE_BODY_KEYS: readonly (keyof Pick<
  SessionMessage,
  'acpRawInput' | 'acpRawOutput' | 'acpContent'
>)[] = ['acpRawInput', 'acpRawOutput', 'acpContent'];

/**
 * A re-fetched tool row with each `$deferred` marker swapped back for the body
 * the pane already holds, where that body is evidently the one the marker
 * stands in for.
 *
 * An in-flight row ships its bodies inline on every poll; the tick that sees
 * it settle gets it back with the large ones deferred. Taking that update as
 * is would turn an expanded card to "Loading details…" and fetch the bytes it
 * just dropped. But the held copy is not necessarily current — streamed tool
 * output accumulates in `acp_content` while the row is in flight, and an
 * update may replace `rawInput` — so a body is only kept when its serialized
 * UTF-8 size equals the marker's `bytes`, the size the store measured on the
 * settled body (`serialized_len` in `store/tool_bodies.rs`). A mismatch,
 * including an in-flight copy whose images are still inline base64 where the
 * settled one carries refs, leaves the marker in place and the card fetches as
 * before. Returns `update` itself when nothing was kept.
 */
export function retainInlineToolBodies(
  current: SessionMessage,
  update: SessionMessage
): SessionMessage {
  if (current.id !== update.id) return update;
  let retained: SessionMessage | null = null;
  for (const key of MESSAGE_BODY_KEYS) {
    const marker = deferredToolBody(update[key]);
    if (!marker || marker.messageId !== update.id || BODY_KEY_BY_FIELD[marker.field] !== key) {
      continue;
    }
    const held = current[key];
    if (
      held == null ||
      deferredToolBody(held) ||
      utf8Length(JSON.stringify(held)) !== marker.bytes
    ) {
      continue;
    }
    retained ??= { ...update };
    retained[key] = held;
  }
  return retained ?? update;
}

function utf8Length(text: string): number {
  return new TextEncoder().encode(text).length;
}

function nonEmptyString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
