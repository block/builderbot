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
