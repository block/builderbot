import type { Comment, CommitTimelineItem, Review } from '../../types';

type ReviewCommit = Pick<CommitTimelineItem, 'order' | 'sha'>;

export function isEmptyFailedReview(params: {
  sessionStatus: string | null;
  sessionId: string | null;
  title: string | null;
  totalCount: number;
}): boolean {
  const { sessionStatus, sessionId, title, totalCount } = params;
  const isRunning = sessionStatus === 'running';
  const isQueued = sessionStatus === 'queued';
  const hasTitle = !!title?.trim();
  return !isRunning && !isQueued && !!sessionId && totalCount === 0 && !hasTitle;
}

export function hasCommitAfterReview(
  reviewCommitSha: string,
  commits: readonly ReviewCommit[]
): boolean {
  const reviewCommit = commits.find((commit) => commit.sha === reviewCommitSha);
  if (!reviewCommit) return false;

  return commits.some((commit) => !!commit.sha && commit.order > reviewCommit.order);
}

export function countUserComments(
  comments: readonly Pick<Comment, 'author' | 'commentType'>[]
): number {
  return comments.filter((comment) => comment.author === 'user').length;
}

export function shouldWarnBeforeDeletingReview(params: {
  review: Pick<Review, 'commitSha'> | null | undefined;
  commits: readonly ReviewCommit[];
  userCommentCount: number | null | undefined;
}): boolean {
  const { review, commits, userCommentCount } = params;

  if (!review?.commitSha) return true;
  if (userCommentCount !== 0) return true;

  return !hasCommitAfterReview(review.commitSha, commits);
}
