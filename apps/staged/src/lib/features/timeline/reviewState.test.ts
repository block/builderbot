import { describe, expect, it } from 'vitest';

import {
  countUserComments,
  hasCommitAfterReview,
  isEmptyFailedReview,
  shouldWarnBeforeDeletingReview,
} from './reviewState';

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

describe('hasCommitAfterReview', () => {
  const commits = [
    { sha: 'first', order: 0 },
    { sha: 'review-anchor', order: 1 },
    { sha: 'newer', order: 2 },
  ];

  it('detects commits after the review anchor', () => {
    expect(hasCommitAfterReview('review-anchor', commits)).toBe(true);
  });

  it('does not detect newer commits when the review anchor is missing', () => {
    expect(hasCommitAfterReview('missing', commits)).toBe(false);
  });
});

describe('shouldWarnBeforeDeletingReview', () => {
  const commits = [
    { sha: 'review-anchor', order: 0 },
    { sha: 'newer', order: 1 },
  ];

  it('skips warning when there is a newer commit and no user comments', () => {
    expect(
      shouldWarnBeforeDeletingReview({
        review: { commitSha: 'review-anchor' },
        commits,
        userCommentCount: 0,
      })
    ).toBe(false);
  });

  it('warns when there is no newer commit', () => {
    expect(
      shouldWarnBeforeDeletingReview({
        review: { commitSha: 'newer' },
        commits,
        userCommentCount: 0,
      })
    ).toBe(true);
  });

  it('warns when the review has user comments', () => {
    expect(
      shouldWarnBeforeDeletingReview({
        review: { commitSha: 'review-anchor' },
        commits,
        userCommentCount: 1,
      })
    ).toBe(true);
  });

  it('warns when the review anchor commit is missing', () => {
    expect(
      shouldWarnBeforeDeletingReview({
        review: { commitSha: 'missing' },
        commits,
        userCommentCount: 0,
      })
    ).toBe(true);
  });

  it('treats agent comments and information annotations as non-user comments', () => {
    const userCommentCount = countUserComments([
      { author: 'agent', commentType: 'issue' },
      { author: 'agent', commentType: 'warning' },
      { author: 'agent', commentType: 'information' },
    ]);

    expect(userCommentCount).toBe(0);
    expect(
      shouldWarnBeforeDeletingReview({
        review: { commitSha: 'review-anchor' },
        commits,
        userCommentCount,
      })
    ).toBe(false);
  });
});
