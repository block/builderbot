/**
 * The one Svelte context entry a pooled pane needs from its adopter: bits-ui's
 * config, which carries `defaultPortalTo="#portal-root"` from Root.svelte.
 * Without it the pane's pickers and viewers portal to <body>, and every open
 * restyles the whole app again. Only that entry is copied — a pooled pane
 * outlives its dialog, and the full context map would keep the first
 * adopter's `DialogRootState` (and the rest of its ancestry) reachable from a
 * parked pane.
 *
 * `BitsConfigContext` is not exported, so the entry is found by shape:
 * `getBitsConfig()` returns `BitsConfigContext.getOr(fallback).opts`, and
 * `BitsConfigState` stores that same object on `this.opts` (bits-ui 2.19.3).
 * A bits-ui that returned a copy, or a tree with no `<BitsConfig>` above the
 * adopter (so `getBitsConfig()` resolved to its fallback), would match nothing;
 * `assertBitsConfigContext` makes that loud in dev rather than a perf cliff.
 */

/** The entries of `contexts` whose state resolves to `config`. */
export function bitsConfigContextEntries(
  contexts: ReadonlyMap<unknown, unknown>,
  config: unknown
): Map<unknown, unknown> {
  const only = new Map<unknown, unknown>();
  for (const [key, value] of contexts) {
    if ((value as { opts?: unknown } | null)?.opts === config) only.set(key, value);
  }
  return only;
}

/**
 * Exactly one entry must have matched: none means the pane will portal to
 * <body>; more than one means the shape match is no longer specific to the
 * config. Throws only in dev — in production the pane still mounts and the
 * pickers merely portal to <body>, which is a regression, not a failure.
 */
export function assertBitsConfigContext(copied: ReadonlyMap<unknown, unknown>, dev: boolean): void {
  if (!dev || copied.size === 1) return;
  throw new Error(
    `PooledSessionChatPane: expected exactly one bits-ui config context entry, found ${copied.size}. ` +
      'Either the adopter has no <BitsConfig> ancestor or bits-ui no longer stores the object ' +
      'getBitsConfig() returns on the config state (see bitsConfigContext.ts).'
  );
}
