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
  /** Provider-level effort: the fallback for models without their own entry. */
  effort?: string;
  /** Effort keyed by the model it was chosen alongside. */
  modelEfforts?: Record<string, string>;
}

/**
 * Patch for one provider's pref: absent fields are untouched, `null` clears.
 * When `effortModel` names the model the effort was chosen alongside, the
 * effort is also recorded (or cleared) under that model.
 */
export interface AcpConfigPrefPatch {
  model?: string | null;
  effort?: string | null;
  effortModel?: string;
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
    if (patch.effortModel) {
      const modelEfforts = { ...next.modelEfforts };
      if (patch.effort === null) delete modelEfforts[patch.effortModel];
      else modelEfforts[patch.effortModel] = patch.effort;
      if (Object.keys(modelEfforts).length === 0) delete next.modelEfforts;
      else next.modelEfforts = modelEfforts;
    }
  }
  return next;
}

/**
 * The persisted effort to restore for a model: the effort last chosen
 * alongside that model when recorded, otherwise the provider-level effort.
 */
export function preferredAcpEffort(
  pref: AcpConfigPref | null | undefined,
  modelId: string | null
): string | null {
  if (!pref) return null;
  if (modelId) {
    const modelEffort = pref.modelEfforts?.[modelId];
    if (modelEffort) return modelEffort;
  }
  return pref.effort ?? null;
}
