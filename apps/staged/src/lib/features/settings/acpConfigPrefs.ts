/**
 * Merge logic for the per-provider ACP config picker preferences persisted
 * by the preferences store. Kept separate from preferences.svelte.ts so it
 * can be unit-tested without the Svelte runtime.
 */

/**
 * The last explicitly chosen picker values for one provider, stored as
 * selector valueIds. Labels come from live discovery options, so persisting
 * ids alone avoids staleness.
 */
export interface AcpConfigPref {
  model?: string;
  effort?: string;
}

/** Patch for one provider's pref: absent fields are untouched, `null` clears. */
export interface AcpConfigPrefPatch {
  model?: string | null;
  effort?: string | null;
}

export function mergeAcpConfigPref(
  current: AcpConfigPref | undefined,
  patch: AcpConfigPrefPatch
): AcpConfigPref {
  const next: AcpConfigPref = { ...current };
  if (patch.model !== undefined) {
    if (patch.model === null) delete next.model;
    else next.model = patch.model;
  }
  if (patch.effort !== undefined) {
    if (patch.effort === null) delete next.effort;
    else next.effort = patch.effort;
  }
  return next;
}
