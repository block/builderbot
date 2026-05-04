import { describe, expect, it } from 'vitest';
import type { BranchTimeline } from '../../types';
import { timelineToHashtagItems } from './hashtagItems';

describe('timelineToHashtagItems', () => {
  it('uses only the commit subject for commit hashtag titles', () => {
    const timeline: BranchTimeline = {
      notes: [],
      commits: [
        {
          id: 'commit-id',
          sha: 'abcdef1234567890',
          shortSha: 'abcdef1',
          subject: 'Add branch picker filtering',
          author: 'Test User',
          timestamp: 1,
          order: 0,
          sessionId: null,
          sessionStatus: null,
          completionReason: null,
        },
      ],
      reviews: [],
      images: [],
    };

    expect(timelineToHashtagItems(timeline)).toContainEqual(
      expect.objectContaining({
        type: 'commit',
        id: 'abcdef1234567890',
        title: 'Add branch picker filtering',
      })
    );
  });
});
