import type { AcpConfigSelector } from '../../api/commands';
import type { AcpConfigSelection, AcpConfigValueSelection } from '../../types';

export interface AcpConfigPickerSelection {
  providerId: string | null;
  acpConfigSelection: AcpConfigSelection | null;
}

interface SelectorSelection {
  selector: AcpConfigSelector | null;
  valueId: string | null;
  explicit?: boolean;
}

interface AcpSelectorSelections {
  model?: SelectorSelection;
  effort?: SelectorSelection;
}

export interface ReconciledSelectorValue {
  valueId: string | null;
  explicit: boolean;
}

/**
 * The value a selector shows when nothing has been chosen: its reported
 * current value when listed, otherwise the first option.
 */
export function defaultSelectorValue(selector: AcpConfigSelector | null): string | null {
  if (!selector || selector.options.length === 0) return null;
  if (selector.options.some((option) => option.valueId === selector.currentValueId)) {
    return selector.currentValueId;
  }
  return selector.options[0]?.valueId ?? null;
}

export function selectorHasValue(
  selector: AcpConfigSelector | null,
  valueId: string | null
): boolean {
  return !!selector && !!valueId && selector.options.some((option) => option.valueId === valueId);
}

/**
 * Reconcile a desired value (a prior explicit choice or persisted preference)
 * against a selector's live options. Keeps the desired value — marked
 * explicit so it is sent at launch — when the selector still offers it;
 * otherwise falls back to the selector default as a display-only value.
 */
export function reconcileSelectorValue(
  selector: AcpConfigSelector | null,
  desiredValueId: string | null
): ReconciledSelectorValue {
  if (selectorHasValue(selector, desiredValueId)) {
    return { valueId: desiredValueId, explicit: true };
  }
  return { valueId: defaultSelectorValue(selector), explicit: false };
}

function selectedValueId(selector: AcpConfigSelector, valueId: string | null): string | null {
  if (valueId && selector.options.some((option) => option.valueId === valueId)) {
    return valueId;
  }
  return null;
}

function valueSelection(selection: SelectorSelection | undefined): AcpConfigValueSelection | null {
  const selector = selection?.selector ?? null;
  if (!selector || !selection?.explicit) return null;

  const valueId = selectedValueId(selector, selection?.valueId ?? null);
  if (!valueId) return null;

  const option = selector.options.find((candidate) => candidate.valueId === valueId);
  return {
    configId: selector.configId,
    valueId,
    label: option?.label ?? null,
  };
}

export function buildAcpConfigSelection(
  selections: AcpSelectorSelections
): AcpConfigSelection | null {
  const model = valueSelection(selections.model);
  const effort = valueSelection(selections.effort);

  if (!model && !effort) return null;
  return { model, effort };
}
