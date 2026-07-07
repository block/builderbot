import { describe, expect, it } from 'vitest';
import type { AcpConfigSelector } from '../../api/commands';
import { buildAcpConfigSelection } from './acpConfigSelection';

function selector(overrides: Partial<AcpConfigSelector> = {}): AcpConfigSelector {
  return {
    configId: 'model',
    label: 'Model',
    currentValueId: 'sonnet',
    options: [
      { valueId: 'sonnet', label: 'Sonnet' },
      { valueId: 'opus', label: 'Opus' },
    ],
    ...overrides,
  };
}

describe('buildAcpConfigSelection', () => {
  it('builds model and effort payloads from selected selector values', () => {
    const model = selector();
    const effort = selector({
      configId: 'reasoning_effort',
      label: 'Effort',
      currentValueId: 'medium',
      options: [
        { valueId: 'medium', label: 'Medium' },
        { valueId: 'high', label: 'High' },
      ],
    });

    expect(
      buildAcpConfigSelection({
        model: { selector: model, valueId: 'opus' },
        effort: { selector: effort, valueId: 'high' },
      })
    ).toEqual({
      model: { configId: 'model', valueId: 'opus', label: 'Opus' },
      effort: { configId: 'reasoning_effort', valueId: 'high', label: 'High' },
    });
  });

  it('falls back to the current selector value when no explicit value is selected', () => {
    expect(
      buildAcpConfigSelection({
        model: { selector: selector(), valueId: null },
      })
    ).toEqual({
      model: { configId: 'model', valueId: 'sonnet', label: 'Sonnet' },
      effort: null,
    });
  });

  it('omits unavailable selectors and returns null when there is no selectable value', () => {
    expect(
      buildAcpConfigSelection({
        model: { selector: selector({ options: [] }), valueId: null },
        effort: { selector: null, valueId: null },
      })
    ).toBeNull();
  });
});
