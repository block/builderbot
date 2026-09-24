import { describe, expect, it } from 'vitest';
import { canStartQueuedSessionNow, type QueuedSessionKind } from './queuedSessionStart';

const allKinds: QueuedSessionKind[] = ['commit', 'note', 'review'];
const readOnlyKinds: QueuedSessionKind[] = ['note', 'review'];

const idle = { commitSessionRunning: false, gitActionRunning: false, provisioning: false };

describe('canStartQueuedSessionNow', () => {
  it('offers Start now for every kind on an idle branch', () => {
    for (const kind of allKinds) {
      expect(canStartQueuedSessionNow({ kind, ...idle })).toBe(true);
    }
  });

  it('offers Start now for notes and reviews while a commit session holds the worktree', () => {
    for (const kind of readOnlyKinds) {
      expect(canStartQueuedSessionNow({ kind, ...idle, commitSessionRunning: true })).toBe(true);
    }
  });

  it('withholds Start now for a commit while a commit session holds the worktree', () => {
    expect(canStartQueuedSessionNow({ kind: 'commit', ...idle, commitSessionRunning: true })).toBe(
      false
    );
  });

  it('withholds Start now for every kind while a git action rewrites the worktree', () => {
    for (const kind of allKinds) {
      expect(canStartQueuedSessionNow({ kind, ...idle, gitActionRunning: true })).toBe(false);
    }
  });

  it('withholds Start now for every kind while a git action runs beside a commit session', () => {
    for (const kind of allKinds) {
      expect(
        canStartQueuedSessionNow({
          kind,
          ...idle,
          commitSessionRunning: true,
          gitActionRunning: true,
        })
      ).toBe(false);
    }
  });

  it('withholds Start now for every kind while the branch is provisioning', () => {
    for (const kind of allKinds) {
      expect(canStartQueuedSessionNow({ kind, ...idle, provisioning: true })).toBe(false);
    }
  });
});
