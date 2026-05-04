// Re-export from shared package
export {
  buildFileEntries,
  buildTree,
  compactTree,
  fileChangeScale,
  fileChangeTotal,
  formatLineRange,
  pathsMatch,
  truncateText,
  type FileEntry,
  type TreeNode,
} from '@builderbot/diff-viewer/utils';

interface GithubCommentUrlComment {
  githubCommentId: number | null;
  githubCommentType: string | null;
}

interface GithubCommentUrlContext {
  prUrl?: string | null;
  githubRepo?: string | null;
  prNumber?: number | null;
}

export function buildGithubCommentUrl(
  comment: GithubCommentUrlComment,
  { prUrl, githubRepo, prNumber }: GithubCommentUrlContext
): string | null {
  if (comment.githubCommentId == null) return null;

  const baseUrl =
    prUrl?.replace(/#.*$/, '') ??
    (githubRepo && prNumber != null ? `https://github.com/${githubRepo}/pull/${prNumber}` : null);

  if (!baseUrl) return null;

  if (comment.githubCommentType === 'review') {
    return `${baseUrl}#discussion_r${comment.githubCommentId}`;
  }
  if (comment.githubCommentType === 'issue') {
    return `${baseUrl}#issuecomment-${comment.githubCommentId}`;
  }

  return null;
}
