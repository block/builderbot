import { describe, expect, it } from 'vitest';
import type { DeferredPushName } from '../../types';
import {
  buildPrMenu,
  deferredPushLabel,
  deferredPushName,
  deferredPushVariant,
  deferredPushWaitingMeta,
  primaryOpensPr,
  type PrMenuEntry,
  type PrMenuInput,
} from './prButtonMenu';

function input(overrides: Partial<PrMenuInput> = {}): PrMenuInput {
  return {
    prState: 'created',
    prStatusState: 'OPEN',
    pushState: 'idle',
    pushAction: 'none',
    prNumber: 42,
    altHeld: false,
    workInFlight: false,
    deferredPush: null,
    deferPushPending: false,
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
    // Deferred pushes: "after current work" rows while work is in flight, one
    // Cancel row while one is armed.
    [
      'PR open, in sync, work in flight',
      { workInFlight: true },
      ['Push after current work', 'Push after current work without verifying', '─', 'Copy PR URL'],
    ],
    [
      'PR open, in sync, work in flight, Option held',
      { workInFlight: true, altHeld: true },
      [
        'Force push after current work',
        'Force push after current work without verifying',
        '─',
        'Copy PR URL',
      ],
    ],
    [
      'PR open, local ahead, work in flight',
      { pushAction: 'push', workInFlight: true },
      [
        'Push without verifying',
        '─',
        'Push after current work',
        'Push after current work without verifying',
        '─',
        'View PR #42',
        'Copy PR URL',
      ],
    ],
    [
      'PR open, local ahead, work in flight, Option held',
      { pushAction: 'push', workInFlight: true, altHeld: true },
      [
        'Force push without verifying',
        '─',
        'Force push after current work',
        'Force push after current work without verifying',
        '─',
        'View PR #42',
        'Copy PR URL',
      ],
    ],
    [
      'PR open, diverged, work in flight (the deferred force pushes confirm too)',
      { pushAction: 'forcePush', workInFlight: true },
      [
        'Force push without verifying… (confirm)',
        '─',
        'Force push after current work… (confirm)',
        'Force push after current work without verifying… (confirm)',
        '─',
        'View PR #42',
        'Copy PR URL',
      ],
    ],
    [
      'PR open, diverged, work in flight, Option held (skips the dialogs)',
      { pushAction: 'forcePush', workInFlight: true, altHeld: true },
      [
        'Force push without verifying',
        '─',
        'Force push after current work',
        'Force push after current work without verifying',
        '─',
        'View PR #42',
        'Copy PR URL',
      ],
    ],
    [
      'PR closed, local ahead, work in flight',
      { prStatusState: 'CLOSED', pushAction: 'push', workInFlight: true },
      [
        'Push without verifying',
        '─',
        'Push after current work',
        'Push after current work without verifying',
        '─',
        'View PR #42',
        'Copy PR URL',
      ],
    ],
    ['PR merged, work in flight', { prStatusState: 'MERGED', workInFlight: true }, ['Copy PR URL']],
    [
      'no PR, work in flight (deferred pushes need a PR, like the push rows)',
      { prState: 'idle', prStatusState: null, prNumber: null, workInFlight: true },
      ['Create draft PR', 'Detect existing PR'],
    ],
    [
      'pushing, work in flight',
      { pushState: 'pushing', pushAction: 'push', workInFlight: true },
      ['View PR #42', 'Copy PR URL'],
    ],
    [
      'PR open, local ahead, deferred push armed',
      { pushAction: 'push', workInFlight: true, deferredPush: 'force-no-verify' },
      ['Push without verifying', '─', 'Cancel deferred push', '─', 'View PR #42', 'Copy PR URL'],
    ],
    [
      'PR open, local ahead, deferred push armed, Option held (Cancel does not swap)',
      { pushAction: 'push', workInFlight: true, deferredPush: 'push', altHeld: true },
      [
        'Force push without verifying',
        '─',
        'Cancel deferred push',
        '─',
        'View PR #42',
        'Copy PR URL',
      ],
    ],
    [
      'PR open, in sync, deferred push armed, work already finished',
      { deferredPush: 'push' },
      ['Cancel deferred push', '─', 'Copy PR URL'],
    ],
    [
      'PR open, diverged, deferred push armed',
      { pushAction: 'forcePush', workInFlight: true, deferredPush: 'force' },
      [
        'Force push without verifying… (confirm)',
        '─',
        'Cancel deferred push',
        '─',
        'View PR #42',
        'Copy PR URL',
      ],
    ],
    [
      'push queued, deferred push armed (the two coexist)',
      { pushState: 'queued', pushAction: 'push', deferredPush: 'push-no-verify' },
      ['Cancel deferred push', '─', 'View PR #42', 'Copy PR URL'],
    ],
    [
      'pushing, deferred push armed',
      { pushState: 'pushing', pushAction: 'push', deferredPush: 'push' },
      ['Cancel deferred push', '─', 'View PR #42', 'Copy PR URL'],
    ],
    [
      'PR merged, deferred push armed',
      { prStatusState: 'MERGED', deferredPush: 'push' },
      ['Cancel deferred push', '─', 'Copy PR URL'],
    ],
    [
      'no PR, deferred push armed (still cancellable after the PR was unlinked)',
      { prState: 'idle', prStatusState: null, prNumber: null, deferredPush: 'force' },
      ['Create draft PR', 'Detect existing PR', '─', 'Cancel deferred push'],
    ],
  ])('%s', (_name, overrides, expected) => {
    expect(labels(buildPrMenu(input(overrides)))).toEqual(expected);
  });

  it('maps each deferred row to its variant, confirming only the diverged force rows', () => {
    const items = (overrides: Partial<PrMenuInput>) =>
      buildPrMenu(input({ workInFlight: true, ...overrides })).filter(
        (entry): entry is Exclude<PrMenuEntry, { id: 'separator' }> =>
          entry.id !== 'separator' && entry.id.startsWith('defer')
      );

    expect(items({ pushAction: 'push' })).toEqual([
      { id: 'deferPush', label: 'Push after current work' },
      { id: 'deferPushNoVerify', label: 'Push after current work without verifying' },
    ]);
    expect(items({ pushAction: 'push', altHeld: true })).toEqual([
      { id: 'deferForcePush', label: 'Force push after current work' },
      { id: 'deferForcePushNoVerify', label: 'Force push after current work without verifying' },
    ]);
    expect(items({ pushAction: 'forcePush' })).toEqual([
      { id: 'deferForcePush', label: 'Force push after current work…', confirm: true },
      {
        id: 'deferForcePushNoVerify',
        label: 'Force push after current work without verifying…',
        confirm: true,
      },
    ]);
    expect(items({ pushAction: 'forcePush', altHeld: true })).toEqual(
      items({ pushAction: 'push', altHeld: true })
    );
    expect(items({})).toEqual(items({ pushAction: 'push' }));
  });

  it('round-trips the four deferred push names through their variants and labels', () => {
    const names: DeferredPushName[] = ['push', 'force', 'push-no-verify', 'force-no-verify'];
    for (const name of names) {
      const { force, noVerify } = deferredPushVariant(name);
      expect(deferredPushName(force, noVerify)).toBe(name);
      expect(deferredPushLabel(name)).toBe(
        `${force ? 'Force push' : 'Push'} after current work${noVerify ? ' without verifying' : ''}`
      );
    }
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

  it('disables the deferred rows, and only those, while an arm or cancel round trip is pending', () => {
    const disabledIds = (overrides: Partial<PrMenuInput>) =>
      buildPrMenu(input({ deferPushPending: true, ...overrides }))
        .filter((entry) => entry.id !== 'separator' && entry.disabled)
        .map((entry) => entry.id);

    expect(disabledIds({ pushAction: 'push', workInFlight: true })).toEqual([
      'deferPush',
      'deferPushNoVerify',
    ]);
    expect(disabledIds({ pushAction: 'forcePush', workInFlight: true, altHeld: true })).toEqual([
      'deferForcePush',
      'deferForcePushNoVerify',
    ]);
    expect(disabledIds({ pushAction: 'push', deferredPush: 'push' })).toEqual([
      'cancelDeferredPush',
    ]);
    // The rows themselves are unchanged apart from the flag: labels, order
    // and the other sections stay as they are.
    expect(labels(buildPrMenu(input({ pushAction: 'push', workInFlight: true })))).toEqual(
      labels(buildPrMenu(input({ pushAction: 'push', workInFlight: true, deferPushPending: true })))
    );
    // Nothing is disabled when nothing is pending.
    for (const entry of buildPrMenu(input({ pushAction: 'push', workInFlight: true }))) {
      if (entry.id !== 'separator') expect(entry.disabled).toBeUndefined();
    }
  });

  it('never starts or ends with a separator, or doubles one', () => {
    const states: Partial<PrMenuInput>[] = [
      { prState: 'idle', prNumber: null },
      { pushAction: 'push' },
      { pushAction: 'forcePush', altHeld: true },
      { pushState: 'queued' },
      {},
      { workInFlight: true },
      { pushAction: 'push', workInFlight: true },
      { pushAction: 'forcePush', workInFlight: true },
      { pushAction: 'forcePush', workInFlight: true, altHeld: true },
      { deferredPush: 'push' },
      { pushAction: 'push', deferredPush: 'force-no-verify' },
      { pushState: 'queued', deferredPush: 'push-no-verify' },
      { prStatusState: 'MERGED', deferredPush: 'push' },
      { prState: 'idle', prNumber: null, deferredPush: 'force' },
      { prState: 'idle', prNumber: null, workInFlight: true },
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

describe('deferredPushWaitingMeta', () => {
  it.each<[Parameters<typeof deferredPushWaitingMeta>[0], string]>([
    [{ branchSessions: 1, projectSessions: 0 }, 'Waiting for 1 branch session to finish'],
    [{ branchSessions: 0, projectSessions: 2 }, 'Waiting for 2 project sessions to finish'],
    [
      { branchSessions: 2, projectSessions: 1 },
      'Waiting for 2 branch sessions and 1 project session to finish',
    ],
    // What this client cannot see (a queued project session, a snapshot not
    // loaded yet) falls back to the generic text rather than "0 sessions".
    [
      { branchSessions: 0, projectSessions: 0 },
      'Waiting for branch and project sessions to finish',
    ],
    [null, 'Waiting for branch and project sessions to finish'],
    [undefined, 'Waiting for branch and project sessions to finish'],
  ])('%o → %s', (waitingOn, expected) => {
    expect(deferredPushWaitingMeta(waitingOn)).toBe(expected);
  });
});
