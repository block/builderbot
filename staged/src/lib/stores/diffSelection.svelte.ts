/**
 * Diff Selection Store
 *
 * Manages the current diff specification (base..head) and presets.
 *
 * Rebuildable: This module owns diff selection state. The rest of the app
 * imports the reactive state directly - no subscriptions needed.
 */

import { DiffSpec } from '../types';

// =============================================================================
// Type Definitions
// =============================================================================

/** Preset diff specifications */
export interface DiffPreset {
  spec: DiffSpec;
  label: string;
}

/**
 * Diff selection state type for factory pattern.
 */
export interface DiffSelection {
  /** Current diff specification */
  spec: DiffSpec;
  /** Label for current selection (preset name or custom) */
  label: string;
  /** PR number if this diff is for a GitHub PR */
  prNumber: number | undefined;
}

/**
 * Preset store type.
 */
export interface PresetStore {
  presets: DiffPreset[];
}

// =============================================================================
// Factory Functions
// =============================================================================

/**
 * Create default presets.
 */
function createDefaultPresets(): DiffPreset[] {
  return [
    { spec: DiffSpec.uncommitted(), label: 'Uncommitted' },
    { spec: DiffSpec.branchChanges(), label: 'Branch Changes' },
    { spec: DiffSpec.lastCommit(), label: 'Last Commit' },
  ];
}

/**
 * Create a new preset store instance.
 * Returns a plain object - caller should wrap with $state() if needed.
 */
export function createPresetStore(): PresetStore {
  return {
    presets: createDefaultPresets(),
  };
}

/**
 * Create a new isolated diff selection state instance.
 * Used by the tab system to create per-tab state.
 * Returns a plain object - caller should wrap with $state() if needed.
 */
export function createDiffSelection(): DiffSelection {
  const presets = createDefaultPresets();
  return {
    spec: presets[0].spec,
    label: presets[0].label,
    prNumber: undefined,
  };
}

// =============================================================================
// Reactive State (Singleton)
// =============================================================================

/**
 * Preset store - wrapped in object because Svelte doesn't allow exporting
 * reassignable $state. Access via `presetStore.presets`.
 */
export const presetStore = $state(createPresetStore());

/** Convenience getter for presets */
export function getPresets(): readonly DiffPreset[] {
  return presetStore.presets;
}

/**
 * Diff selection state object.
 * Use this directly in components - it's reactive!
 * Will be replaced by activeTab.diffSelection in Phase 4.
 */
export const diffSelection = $state(createDiffSelection());

// =============================================================================
// Derived State (as getters)
// =============================================================================

/** Whether current spec matches a preset */
export function isPreset(): boolean {
  return presetStore.presets.some(
    (p) =>
      DiffSpec.display(p.spec) === DiffSpec.display(diffSelection.spec) &&
      p.label === diffSelection.label
  );
}

/** Display label - preset name or "base..head" */
export function getDisplayLabel(): string {
  return diffSelection.label;
}

// =============================================================================
// Actions
// =============================================================================

/**
 * Select a preset.
 */
export function selectPreset(preset: DiffPreset): void {
  diffSelection.spec = preset.spec;
  diffSelection.label = preset.label;
}

/**
 * Select a custom diff specification.
 */
export function selectCustomDiff(spec: DiffSpec, label?: string, prNumber?: number): void {
  diffSelection.spec = spec;
  diffSelection.label = label ?? DiffSpec.display(spec);
  diffSelection.prNumber = prNumber;
}

/**
 * Reset diff selection to "Uncommitted" (first preset).
 * Call when switching repositories.
 */
export function resetDiffSelection(): void {
  diffSelection.spec = presetStore.presets[0].spec;
  diffSelection.label = presetStore.presets[0].label;
  diffSelection.prNumber = undefined;
}
