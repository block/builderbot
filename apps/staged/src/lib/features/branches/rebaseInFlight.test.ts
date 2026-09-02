import { describe, expect, it } from 'vitest';
import { rebaseInFlight } from './rebaseInFlight';

describe('rebaseInFlight', () => {
  it('reports a queued rebase pipeline', () => {
    expect(rebaseInFlight([{ pipelineKind: 'rebase', sessionStatus: 'queued' }])).toBe(true);
  });

  it('reports a running rebase pipeline', () => {
    expect(rebaseInFlight([{ pipelineKind: 'rebase', sessionStatus: 'running' }])).toBe(true);
  });

  it('ignores a rebase that already finished', () => {
    expect(rebaseInFlight([{ pipelineKind: 'rebase', sessionStatus: 'completed' }])).toBe(false);
  });

  it('ignores other active pipelines and plain commit sessions', () => {
    expect(
      rebaseInFlight([
        { pipelineKind: 'squash', sessionStatus: 'running' },
        { pipelineKind: null, sessionStatus: 'running' },
        { sessionStatus: 'queued' },
      ])
    ).toBe(false);
  });

  it('handles a missing timeline', () => {
    expect(rebaseInFlight(undefined)).toBe(false);
  });
});
