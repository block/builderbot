import { describe, expect, it } from 'vitest';
import { buildPrMenu, primaryOpensPr, type PrMenuEntry, type PrMenuInput } from './prButtonMenu';

function input(overrides: Partial<PrMenuInput> = {}): PrMenuInput {
  return {
    prState: 'created',
    prStatusState: 'OPEN',
    pushState: 'idle',
    pushAction: 'none',
    prNumber: 42,
    altHeld: false,
    ...overrides,
  };
}

/** Labels top to bottom, with separators as `─` and confirm items marked `(confirm)`. */
function labels(entries: PrMenuEntry[]): string[] {
  return entries.map((entry) =>
    entry.id === 'separator' ? '─' : `${entry.label}${entry.confirm ? ' (confirm)' : ''}`
  );
}

describe('buildPrMenu', () => {
  it.each<[string, Partial<PrMenuInput>, string[]]>([
    [
      'no PR',
      { prState: 'idle', prStatusState: null, prNumber: null },
      ['Create draft PR', 'Detect existing PR'],
    ],
    [
      'no PR, Option held (the button itself turns into Create draft PR)',
      { prState: 'idle', prStatusState: null, prNumber: null, altHeld: true },
      ['Create draft PR', 'Detect existing PR'],
    ],
    ['PR creating', { prState: 'creating', prStatusState: null, prNumber: null }, []],
    ['PR creation failed', { prState: 'error', prStatusState: null, prNumber: null }, []],
    [
      'no PR, a push from the timeline in flight',
      { prState: 'idle', prStatusState: null, prNumber: null, pushState: 'pushing' },
      [],
    ],
    [
      'PR open, local ahead',
      { pushAction: 'push' },
      ['Push without verifying', '─', 'View PR #42', 'Copy PR URL'],
    ],
    [
      'PR open, local ahead, Option held',
      { pushAction: 'push', altHeld: true },
      ['Force push without verifying', '─', 'View PR #42', 'Copy PR URL'],
    ],
    [
      'PR open, diverged',
      { pushAction: 'forcePush' },
      ['Force push without verifying… (confirm)', '─', 'View PR #42', 'Copy PR URL'],
    ],
    [
      'PR open, diverged, Option held (skips the dialog)',
      { pushAction: 'forcePush', altHeld: true },
      ['Force push without verifying', '─', 'View PR #42', 'Copy PR URL'],
    ],
    ['PR open, in sync (the button opens the PR)', {}, ['Copy PR URL']],
    ['PR open, in sync, Option held', { altHeld: true }, ['Copy PR URL']],
    ['PR merged', { prStatusState: 'MERGED' }, ['Copy PR URL']],
    [
      'PR merged while git state still reports unpushed commits',
      { prStatusState: 'MERGED', pushAction: 'push' },
      ['View PR #42', 'Copy PR URL'],
    ],
    [
      'PR closed, local ahead (the button still offers a push)',
      { prStatusState: 'CLOSED', pushAction: 'push' },
      ['Push without verifying', '─', 'View PR #42', 'Copy PR URL'],
    ],
    ['push queued', { pushState: 'queued', pushAction: 'push' }, ['View PR #42', 'Copy PR URL']],
    ['pushing', { pushState: 'pushing', pushAction: 'push' }, ['View PR #42', 'Copy PR URL']],
    [
      'push failed',
      { pushState: 'error', pushAction: 'push', altHeld: true },
      ['View PR #42', 'Copy PR URL'],
    ],
    [
      'push done, git state not refreshed yet (the button opens the PR)',
      { pushState: 'done', pushAction: 'push' },
      ['Copy PR URL'],
    ],
    [
      'PR known only by URL',
      { prNumber: null, pushAction: 'push' },
      ['Push without verifying', '─', 'View PR', 'Copy PR URL'],
    ],
  ])('%s', (_name, overrides, expected) => {
    expect(labels(buildPrMenu(input(overrides)))).toEqual(expected);
  });

  it('maps both no-verify force labels to one action that only confirms without Option', () => {
    const [diverged] = buildPrMenu(input({ pushAction: 'forcePush' }));
    const [divergedAlt] = buildPrMenu(input({ pushAction: 'forcePush', altHeld: true }));
    const [aheadAlt] = buildPrMenu(input({ pushAction: 'push', altHeld: true }));

    expect(diverged).toEqual({
      id: 'forcePushNoVerify',
      label: 'Force push without verifying…',
      confirm: true,
    });
    expect(divergedAlt).toEqual({ id: 'forcePushNoVerify', label: 'Force push without verifying' });
    expect(aheadAlt).toEqual(divergedAlt);
  });

  it('never starts or ends with a separator, or doubles one', () => {
    const states: Partial<PrMenuInput>[] = [
      { prState: 'idle', prNumber: null },
      { pushAction: 'push' },
      { pushAction: 'forcePush', altHeld: true },
      { pushState: 'queued' },
      {},
    ];
    for (const state of states) {
      const ids = buildPrMenu(input(state)).map((entry) => entry.id);
      expect(ids[0]).not.toBe('separator');
      expect(ids.at(-1)).not.toBe('separator');
      expect(ids.join(',')).not.toContain('separator,separator');
    }
  });
});

describe('primaryOpensPr', () => {
  it.each<[Partial<PrMenuInput>, boolean]>([
    [{}, true],
    [{ pushState: 'done', pushAction: 'push' }, true],
    [{ pushAction: 'push' }, false],
    [{ pushAction: 'forcePush' }, false],
    [{ pushState: 'queued' }, false],
    [{ pushState: 'pushing' }, false],
    [{ pushState: 'error' }, false],
    [{ prState: 'idle' }, false],
    [{ prState: 'creating' }, false],
  ])('%o → %s', (overrides, expected) => {
    expect(primaryOpensPr(input(overrides))).toBe(expected);
  });
});
