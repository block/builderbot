import { describe, expect, it } from 'vitest';
import { buildPrButtonTitle } from './prButtonTooltip';
import type { PrFailedCheck } from '../../types';

const NOW = new Date('2026-05-05T12:00:00Z').getTime();

function title(overrides: Partial<Parameters<typeof buildPrButtonTitle>[0]> = {}): string {
  return buildPrButtonTitle({
    actionTitle: 'View PR #123',
    prNumber: 123,
    prHeadSha: 'abcdef1234567890',
    prFetchedAt: NOW - 4 * 60_000,
    checksStatus: 'SUCCESS',
    statusStale: false,
    statusRefreshing: false,
    hasUnpushed: false,
    failedChecks: [],
    statusCleared: false,
    nowMs: NOW,
    ...overrides,
  });
}

describe('buildPrButtonTitle', () => {
  it('formats succeeded checks', () => {
    expect(title()).toBe(
      ['View PR #123', 'Latest PR SHA: abcdef1', 'Last checked: 4m ago', 'Checks: succeeded'].join(
        '\n'
      )
    );
  });

  it('formats failed checks with failed check details', () => {
    const failedChecks: PrFailedCheck[] = [
      { name: 'unit-tests', state: 'FAILURE', detailsUrl: 'https://github.com/checks/1' },
      { name: 'lint', state: 'TIMED_OUT', detailsUrl: null },
    ];

    expect(
      title({
        actionTitle: 'Checks failing',
        checksStatus: 'FAILURE',
        failedChecks,
      })
    ).toBe(
      [
        'Checks failing',
        'Latest PR SHA: abcdef1',
        'Last checked: 4m ago',
        'Checks: failed',
        'Failed: unit-tests (FAILURE), lint (TIMED_OUT)',
      ].join('\n')
    );
  });

  it('formats pending checks', () => {
    expect(
      title({
        actionTitle: 'Checks pending',
        checksStatus: 'PENDING',
      })
    ).toBe(
      ['Checks pending', 'Latest PR SHA: abcdef1', 'Last checked: 4m ago', 'Checks: pending'].join(
        '\n'
      )
    );
  });

  it('calls out stale polling', () => {
    expect(
      title({
        actionTitle: 'Checks failing',
        checksStatus: 'FAILURE',
        statusStale: true,
      })
    ).toBe(
      [
        'Checks failing',
        'Latest PR SHA: abcdef1',
        'Last checked: 4m ago',
        'Checks: failed',
        'Status refresh may be outdated',
      ].join('\n')
    );
  });

  it('calls out a refresh in progress', () => {
    expect(
      title({
        statusRefreshing: true,
      })
    ).toBe(
      [
        'View PR #123',
        'Latest PR SHA: abcdef1',
        'Last checked: 4m ago',
        'Status refresh in progress',
        'Checks: succeeded',
      ].join('\n')
    );
  });

  it('formats unknown status fields', () => {
    expect(
      title({
        prHeadSha: null,
        prFetchedAt: null,
        checksStatus: null,
      })
    ).toBe(
      ['View PR #123', 'Latest PR SHA: unknown', 'Last checked: unknown', 'Checks: unknown'].join(
        '\n'
      )
    );
  });

  it('explains when local commits are newer than the PR', () => {
    expect(
      title({
        actionTitle: 'Push changes to remote',
        prFetchedAt: NOW - 12 * 60_000,
        hasUnpushed: true,
      })
    ).toBe(
      [
        'Push changes to remote',
        'Latest PR SHA: abcdef1',
        'Local branch has newer commits than the PR',
        'Last checked: 12m ago',
      ].join('\n')
    );
  });

  it('explains when previous PR status was cleared after a push', () => {
    expect(
      title({
        prFetchedAt: null,
        checksStatus: null,
        failedChecks: [{ name: 'unit-tests', state: 'FAILURE', detailsUrl: null }],
        statusCleared: true,
      })
    ).toBe(
      [
        'View PR #123',
        'Latest PR SHA: abcdef1',
        'Last checked: unknown',
        'Previous PR status was cleared; waiting for next GitHub refresh',
      ].join('\n')
    );
  });
});
