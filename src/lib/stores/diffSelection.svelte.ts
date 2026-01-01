/**
 * Diff Selection Store
 *
 * Manages the current diff specification (base..head) and presets.
 * Handles SHA resolution for tooltip display.
 *
 * Rebuildable: This module owns diff selection state. The rest of the app
 * imports the reactive state directly - no subscriptions needed.
 */

import { resolveRef } from '../services/git';
import type { DiffSpec } from '../types';

// =============================================================================
// Presets
// =============================================================================

/** Available diff presets */
export const DIFF_PRESETS: readonly DiffSpec[] = [
  { base: 'HEAD', head: '@', label: 'Working Changes' },
  { base: 'main', head: '@', label: 'Against main' },
  { base: 'HEAD~1', head: 'HEAD', label: 'Last Commit' },
];

// =============================================================================
// Reactive State
// =============================================================================

/**
 * Diff selection state object.
 * Use this directly in components - it's reactive!
 *
 * Note: We use an object wrapper because Svelte doesn't allow exporting
 * reassignable $state. By mutating properties instead, reactivity works
 * across module boundaries.
 */
export const diffSelection = $state({
  /** Current diff specification */
  spec: DIFF_PRESETS[0] as DiffSpec,
  /** Resolved SHA for base ref (for tooltip display) */
  resolvedBaseSha: null as string | null,
  /** Resolved SHA for head ref (for tooltip display) */
  resolvedHeadSha: null as string | null,
});

// =============================================================================
// Derived State (as getters - Svelte doesn't allow exporting $derived)
// =============================================================================

/** Whether current spec matches a preset */
export function isPreset(): boolean {
  return DIFF_PRESETS.some(
    (p) =>
      p.base === diffSelection.spec.base &&
      p.head === diffSelection.spec.head &&
      p.label === diffSelection.spec.label
  );
}

/** Display label - preset name or "base..head" */
export function getDisplayLabel(): string {
  return isPreset()
    ? diffSelection.spec.label
    : `${diffSelection.spec.base}..${diffSelection.spec.head}`;
}

/** Tooltip showing resolved SHAs */
export function getTooltipText(): string {
  const basePart = diffSelection.resolvedBaseSha
    ? `${diffSelection.spec.base} (${diffSelection.resolvedBaseSha})`
    : diffSelection.spec.base;
  const headPart = diffSelection.resolvedHeadSha
    ? `${diffSelection.spec.head} (${diffSelection.resolvedHeadSha})`
    : diffSelection.spec.head;
  return `${basePart} → ${headPart}`;
}

// =============================================================================
// Actions
// =============================================================================

/**
 * Update resolved SHAs for the current diff spec.
 */
async function updateResolvedShas(): Promise<void> {
  try {
    diffSelection.resolvedBaseSha = await resolveRef(diffSelection.spec.base);
    diffSelection.resolvedHeadSha = await resolveRef(diffSelection.spec.head);
  } catch {
    diffSelection.resolvedBaseSha = null;
    diffSelection.resolvedHeadSha = null;
  }
}

/**
 * Select a diff specification.
 * Resolves SHAs - reactivity handles the rest.
 */
export async function selectDiffSpec(spec: DiffSpec): Promise<void> {
  diffSelection.spec = spec;
  await updateResolvedShas();
}

/**
 * Select a diff by base and head refs (creates a custom spec).
 */
export async function selectCustomDiff(base: string, head: string, label?: string): Promise<void> {
  await selectDiffSpec({
    base,
    head,
    label: label ?? `${base}..${head}`,
  });
}

/**
 * Initialize the diff selection (resolve initial SHAs).
 * Call once on app startup.
 */
export async function initDiffSelection(): Promise<void> {
  await updateResolvedShas();
}
