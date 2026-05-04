import { describe, expect, it } from 'vitest';
import { formatPipelineStepDuration } from './pipelineDuration';

describe('formatPipelineStepDuration', () => {
  it('returns an empty string before a step has started', () => {
    expect(formatPipelineStepDuration(null, null, 1_000)).toBe('');
  });

  it('formats sub-minute durations in seconds', () => {
    expect(formatPipelineStepDuration(1_000, 44_000, 44_000)).toBe('43s');
  });

  it('formats durations longer than a minute with minutes and seconds', () => {
    expect(formatPipelineStepDuration(0, 101_000, 101_000)).toBe('1m 41s');
  });

  it('formats durations longer than an hour with hours, minutes, and seconds as needed', () => {
    expect(formatPipelineStepDuration(1_000, 3_724_000, 3_724_000)).toBe('1h 2m 3s');
    expect(formatPipelineStepDuration(1_000, 3_601_000, 3_601_000)).toBe('1h');
  });

  it('uses current time while a step is still running', () => {
    expect(formatPipelineStepDuration(1_000, null, 62_000)).toBe('1m 1s');
  });
});
