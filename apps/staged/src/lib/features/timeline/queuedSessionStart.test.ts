import { describe, expect, it } from 'vitest';
import { canStartQueuedSessionNow, type QueuedSessionKind } from './queuedSessionStart';

const allKinds: QueuedSessionKind[] = ['commit', 'note', 'review'];

describe('canStartQueuedSessionNow', () => {
  it('offers Start now for every kind on an idle branch', () => {
    for (const kind of allKinds) {
      expect(
        canStartQueuedSessionNow({ kind, exclusiveHolderRunning: false, provisioning: false })
      ).toBe(true);
    }
  });

  it('offers Start now for notes and reviews while a commit or git action holds the worktree', () => {
    for (const kind of ['note', 'review'] as QueuedSessionKind[]) {
      expect(
        canStartQueuedSessionNow({ kind, exclusiveHolderRunning: true, provisioning: false })
      ).toBe(true);
    }
  });

  it('withholds Start now for a commit while a commit or git action holds the worktree', () => {
    expect(
      canStartQueuedSessionNow({
        kind: 'commit',
        exclusiveHolderRunning: true,
        provisioning: false,
      })
    ).toBe(false);
  });

  it('withholds Start now for every kind while the branch is provisioning', () => {
    for (const kind of allKinds) {
      expect(
        canStartQueuedSessionNow({ kind, exclusiveHolderRunning: false, provisioning: true })
      ).toBe(false);
    }
  });
});
