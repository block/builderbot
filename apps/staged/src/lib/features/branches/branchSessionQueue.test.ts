import { describe, expect, it } from 'vitest';
import { shouldQueueBranchSession, type BranchSessionQueueTimeline } from './branchSessionQueue';
import type { BranchSessionType } from '../../types';

function timeline(
  sessions: Partial<Record<BranchSessionType, string | null>> = {},
  autoReviewStatus: string | null = null
): BranchSessionQueueTimeline {
  return {
    commits: sessions.commit === undefined ? [] : [{ sessionStatus: sessions.commit }],
    notes: sessions.note === undefined ? [] : [{ sessionStatus: sessions.note }],
    reviews: [
      ...(sessions.review === undefined ? [] : [{ sessionStatus: sessions.review, isAuto: false }]),
      ...(autoReviewStatus === null ? [] : [{ sessionStatus: autoReviewStatus, isAuto: true }]),
    ],
  };
}

describe('shouldQueueBranchSession', () => {
  it('allows a note to start while a review is running', () => {
    expect(
      shouldQueueBranchSession({
        mode: 'note',
        timeline: timeline({ review: 'running' }),
      })
    ).toBe(false);
  });

  it('allows a review to start while a note is running', () => {
    expect(
      shouldQueueBranchSession({
        mode: 'review',
        timeline: timeline({ note: 'running' }),
      })
    ).toBe(false);
  });

  it('queues a commit while a note or review is running', () => {
    expect(
      shouldQueueBranchSession({
        mode: 'commit',
        timeline: timeline({ note: 'running' }),
      })
    ).toBe(true);
    expect(
      shouldQueueBranchSession({
        mode: 'commit',
        timeline: timeline({ review: 'running' }),
      })
    ).toBe(true);
  });

  it('queues new work behind any queued user session', () => {
    for (const queuedMode of ['commit', 'note', 'review'] satisfies BranchSessionType[]) {
      for (const mode of ['commit', 'note', 'review'] satisfies BranchSessionType[]) {
        expect(
          shouldQueueBranchSession({
            mode,
            timeline: timeline({ [queuedMode]: 'queued' }),
          })
        ).toBe(true);
      }
    }
  });

  it('ignores auto reviews when deciding whether user work can start', () => {
    for (const mode of ['commit', 'note', 'review'] satisfies BranchSessionType[]) {
      expect(
        shouldQueueBranchSession({
          mode,
          timeline: timeline({}, 'running'),
        })
      ).toBe(false);
    }
  });

  it('allows same-type notes but queues same-type reviews', () => {
    expect(
      shouldQueueBranchSession({
        mode: 'note',
        timeline: timeline({ note: 'running' }),
      })
    ).toBe(false);
    expect(
      shouldQueueBranchSession({
        mode: 'review',
        timeline: timeline({ review: 'running' }),
      })
    ).toBe(true);
  });

  it('queues while the timeline or an optimistic session start is pending', () => {
    expect(shouldQueueBranchSession({ mode: 'note', timeline: null })).toBe(true);
    expect(
      shouldQueueBranchSession({
        mode: 'review',
        timeline: timeline(),
        hasPendingSessionStart: true,
      })
    ).toBe(true);
    expect(
      shouldQueueBranchSession({
        mode: 'review',
        timeline: timeline(),
        hasPendingQueuedSession: true,
      })
    ).toBe(true);
  });
});
