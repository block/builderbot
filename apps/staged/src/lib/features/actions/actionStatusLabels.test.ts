import { describe, expect, it } from 'vitest';
import { actionStatusLabels } from './actionStatusLabels';

const idle = { actionName: 'Dev', stopping: false, showStop: false, running: false } as const;

describe('actionStatusLabels', () => {
  it('names the action when it has never run', () => {
    expect(actionStatusLabels(idle)).toEqual({ title: 'Dev', ariaLabel: 'Dev' });
  });

  it('reports the outcome of a finished execution', () => {
    expect(actionStatusLabels({ ...idle, status: 'completed' })).toEqual({
      title: 'Dev completed',
      ariaLabel: 'Dev completed',
    });
    expect(actionStatusLabels({ ...idle, status: 'failed' })).toEqual({
      title: 'Dev failed',
      ariaLabel: 'Dev failed',
    });
  });

  it('names the action again once a stopped execution is the only history', () => {
    expect(actionStatusLabels({ ...idle, status: 'stopped' })).toEqual({
      title: 'Dev',
      ariaLabel: 'Dev',
    });
  });

  it('points a live execution at its output', () => {
    expect(actionStatusLabels({ ...idle, running: true, status: 'running' })).toEqual({
      title: 'View output for Dev',
      ariaLabel: 'View output for Dev',
    });
  });

  it('offers to stop while the alt affordance is showing', () => {
    expect(
      actionStatusLabels({ ...idle, showStop: true, running: true, status: 'running' })
    ).toEqual({ title: 'Stop Dev', ariaLabel: 'Stop Dev' });
  });

  it('drops the tooltip ellipsis from the accessible name while stopping', () => {
    expect(
      actionStatusLabels({ ...idle, stopping: true, running: true, status: 'running' })
    ).toEqual({ title: 'Stopping…', ariaLabel: 'Stopping' });
  });

  it('prefers the more specific rung when several apply', () => {
    // Stopping wins over the stop affordance, which wins over "view output",
    // which wins over a status that has already been superseded.
    expect(
      actionStatusLabels({
        ...idle,
        stopping: true,
        showStop: true,
        running: true,
        status: 'completed',
      }).title
    ).toBe('Stopping…');
    expect(
      actionStatusLabels({ ...idle, showStop: true, running: true, status: 'completed' }).title
    ).toBe('Stop Dev');
    expect(actionStatusLabels({ ...idle, running: true, status: 'completed' }).title).toBe(
      'View output for Dev'
    );
  });
});
