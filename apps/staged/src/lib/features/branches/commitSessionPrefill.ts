export function getCommitPrefillFromReviewComments(userVisibleCommentCount: number): string {
  return userVisibleCommentCount > 0 ? 'Resolve code review comments' : '';
}
