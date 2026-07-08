import type {
  AcpConfigDiscovery,
  AcpConfigSelector,
  AcpConfigValueOption,
} from '../../api/commands';
import type { SessionMessage } from '../../types';

const MODEL_CATEGORY = 'model';
const EFFORT_CATEGORY = 'thought_level';

export function latestAcpConfigDiscoveryFromMetadata(
  providerId: string | null | undefined,
  metadataMessages: SessionMessage[]
): AcpConfigDiscovery | null {
  if (!providerId) return null;

  const latest = [...metadataMessages]
    .reverse()
    .find((message) => message.acpEventKind === 'config_options_update');
  const options = arrayValue(latest?.acpConfigOptions ?? latest?.acpContent);
  if (!options) return null;

  return {
    providerId,
    model: normalizeSelectorForCategory(options, MODEL_CATEGORY),
    effort: normalizeSelectorForCategory(options, EFFORT_CATEGORY),
  };
}

function normalizeSelectorForCategory(
  configOptions: unknown[],
  category: string
): AcpConfigSelector | null {
  for (const option of configOptions) {
    const record = recordValue(option);
    if (
      !record ||
      stringValue(record.category) !== category ||
      stringValue(record.type) !== 'select'
    ) {
      continue;
    }

    const configId = stringValue(record.id);
    const currentValueId = stringValue(record.currentValue);
    if (!configId || !currentValueId) continue;

    return {
      configId,
      label: stringValue(record.name) ?? configId,
      currentValueId,
      options: flattenOptions(arrayValue(record.options) ?? []),
    };
  }
  return null;
}

function flattenOptions(
  options: unknown[],
  groupLabel: string | null = null
): AcpConfigValueOption[] {
  const flattened: AcpConfigValueOption[] = [];
  for (const option of options) {
    const record = recordValue(option);
    if (!record) continue;

    const nestedOptions = arrayValue(record.options);
    const valueId = stringValue(record.value);
    if (nestedOptions && !valueId) {
      flattened.push(...flattenOptions(nestedOptions, stringValue(record.name)));
      continue;
    }

    if (!valueId) continue;
    flattened.push({
      valueId,
      label: stringValue(record.name) ?? valueId,
      groupLabel,
    });
  }
  return flattened;
}

function recordValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function arrayValue(value: unknown): unknown[] | null {
  return Array.isArray(value) ? value : null;
}

function stringValue(value: unknown): string | null {
  return typeof value === 'string' ? value : null;
}
