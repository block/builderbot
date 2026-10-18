import { describe, expect, it } from 'vitest';

import { reviewBadges } from './reviewBadges';

describe('reviewBadges', () => {
  it('splits warnings from other comments and ignores information annotations', () => {
    // 3 issues + 2 warnings + 4 information notes.
    expect(reviewBadges({ commentCount: 5, annotationCount: 4, warningCount: 2 })).toEqual([
      { icon: 'warning', count: 2 },
      { icon: 'comment', count: 3 },
    ]);
  });

  it('renders a single comment badge when there are no warnings', () => {
    expect(reviewBadges({ commentCount: 3, annotationCount: 1, warningCount: 0 })).toEqual([
      { icon: 'comment', count: 3 },
    ]);
  });

  it('renders only a warning badge when every comment is a warning', () => {
    expect(reviewBadges({ commentCount: 2, annotationCount: 0, warningCount: 2 })).toEqual([
      { icon: 'warning', count: 2 },
    ]);
  });

  it('renders no badges for a review with only information annotations', () => {
    expect(reviewBadges({ commentCount: 0, annotationCount: 4, warningCount: 0 })).toEqual([]);
  });
});
