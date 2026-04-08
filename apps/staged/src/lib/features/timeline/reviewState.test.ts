import { describe, expect, it } from 'vitest';

import { isEmptyFailedReview } from './reviewState';

describe('isEmptyFailedReview', () => {
  it('treats a titled review with no comments as successful', () => {
    expect(
      isEmptyFailedReview({
        sessionStatus: 'completed',
        sessionId: 'session-1',
        title: 'Reasonably confident, but local CI could not be verified from this sandbox',
        totalCount: 0,
      })
    ).toBe(false);
  });

  it('treats a review with no title and no comments as failed', () => {
    expect(
      isEmptyFailedReview({
        sessionStatus: 'completed',
        sessionId: 'session-1',
        title: null,
        totalCount: 0,
      })
    ).toBe(true);
  });
});
