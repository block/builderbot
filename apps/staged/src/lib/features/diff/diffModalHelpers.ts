// Re-export from shared package
export {
  buildFileEntries,
  buildTree,
  compactTree,
  fileChangeScale,
  fileChangeTotal,
  formatLineRange,
  resolveCommentFilePath,
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

/**
 * Serialises a map of CSS custom properties to inline-style declarations
 * (`--prop: value;` each), in insertion order. Used to fold the diff theme's
 * variables into the diff-viewer container's Svelte-owned `style` string:
 * Svelte 5 applies a changed bound style by assigning `cssText`, which
 * replaces the whole inline style, so anything set on the element separately
 * (with `style.setProperty`) is lost the first time the bound string changes.
 */
export function cssCustomPropertyDeclarations(vars: Record<string, string>): string {
  let declarations = '';
  for (const [prop, value] of Object.entries(vars)) {
    declarations += `${prop}: ${value};`;
  }
  return declarations;
}
