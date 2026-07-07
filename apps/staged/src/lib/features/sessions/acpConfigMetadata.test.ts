import { describe, expect, it } from 'vitest';
import type { SessionMessage } from '../../types';
import { latestAcpConfigDiscoveryFromMetadata } from './acpConfigMetadata';

function metadataMessage(
  id: number,
  acpConfigOptions: unknown,
  acpContent?: unknown
): SessionMessage {
  return {
    id,
    sessionId: 'session-1',
    role: 'assistant',
    content: '',
    createdAt: id,
    acpEventKind: 'config_options_update',
    acpConfigOptions,
    acpContent,
  };
}

describe('latestAcpConfigDiscoveryFromMetadata', () => {
  it('normalizes the latest model and effort selectors from ACP metadata', () => {
    const result = latestAcpConfigDiscoveryFromMetadata('codex', [
      metadataMessage(1, [
        {
          id: 'model',
          name: 'Model',
          category: 'model',
          type: 'select',
          currentValue: 'sonnet',
          options: [{ value: 'sonnet', name: 'Sonnet' }],
        },
      ]),
      metadataMessage(2, [
        {
          id: 'model',
          name: 'Model',
          category: 'model',
          type: 'select',
          currentValue: 'opus',
          options: [
            { value: 'sonnet', name: 'Sonnet' },
            { value: 'opus', name: 'Opus' },
          ],
        },
        {
          id: 'reasoning',
          name: 'Effort',
          category: 'thought_level',
          type: 'select',
          currentValue: 'high',
          options: [
            { value: 'low', name: 'Low' },
            { value: 'high', name: 'High' },
          ],
        },
      ]),
    ]);

    expect(result).toEqual({
      providerId: 'codex',
      model: {
        configId: 'model',
        label: 'Model',
        currentValueId: 'opus',
        options: [
          { valueId: 'sonnet', label: 'Sonnet', groupLabel: null },
          { valueId: 'opus', label: 'Opus', groupLabel: null },
        ],
      },
      effort: {
        configId: 'reasoning',
        label: 'Effort',
        currentValueId: 'high',
        options: [
          { valueId: 'low', label: 'Low', groupLabel: null },
          { valueId: 'high', label: 'High', groupLabel: null },
        ],
      },
    });
  });

  it('flattens grouped options and ignores unsupported config categories', () => {
    const result = latestAcpConfigDiscoveryFromMetadata('goose', [
      metadataMessage(1, [
        {
          id: 'mode',
          name: 'Mode',
          category: 'mode',
          type: 'select',
          currentValue: 'default',
          options: [{ value: 'default', name: 'Default' }],
        },
        {
          id: 'model',
          name: 'Model',
          category: 'model',
          type: 'select',
          currentValue: 'pro',
          options: [
            {
              name: 'Fast',
              options: [{ value: 'flash', name: 'Flash' }],
            },
            {
              name: 'Deep',
              options: [{ value: 'pro', name: 'Pro' }],
            },
          ],
        },
      ]),
    ]);

    expect(result?.model?.options).toEqual([
      { valueId: 'flash', label: 'Flash', groupLabel: 'Fast' },
      { valueId: 'pro', label: 'Pro', groupLabel: 'Deep' },
    ]);
    expect(result?.effort).toBeNull();
  });

  it('falls back to acpContent and returns null when metadata is absent', () => {
    expect(latestAcpConfigDiscoveryFromMetadata('goose', [])).toBeNull();
    expect(
      latestAcpConfigDiscoveryFromMetadata('goose', [
        metadataMessage(1, undefined, [
          {
            id: 'model',
            name: 'Model',
            category: 'model',
            type: 'select',
            currentValue: 'default',
            options: [{ value: 'default', name: 'Default' }],
          },
        ]),
      ])?.model?.currentValueId
    ).toBe('default');
  });
});
