import type { ReviewTimelineItem } from '../../types';
import type { TimelineBadge } from './TimelineRow.svelte';

type ReviewCommentCounts = Pick<
  ReviewTimelineItem,
  'commentCount' | 'annotationCount' | 'warningCount'
>;

/**
 * Count badges for a review row, straight from the timeline payload.
 *
 * Warnings get their own badge; every other visible comment is a plain
 * comment. `information` comments are hold-to-reveal annotations and never
 * show up as a badge.
 */
export function reviewBadges(review: ReviewCommentCounts): TimelineBadge[] {
  const nonWarningCount = review.commentCount - review.warningCount;

  const badges: TimelineBadge[] = [];
  if (review.warningCount > 0) {
    badges.push({ icon: 'warning', count: review.warningCount });
  }
  if (nonWarningCount > 0) {
    badges.push({ icon: 'comment', count: nonWarningCount });
  }
  return badges;
}
