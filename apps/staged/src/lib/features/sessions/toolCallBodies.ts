import type { ToolCallBody } from '../../types';
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

function nonEmptyString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
