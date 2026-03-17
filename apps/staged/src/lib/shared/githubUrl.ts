export interface RepoSelection {
  nameWithOwner: string;
  subpath?: string;
  prNumber?: number;
  branchName?: string;
}

/**
 * Parse a GitHub URL into a RepoSelection.
 *
 * Supports:
 * - PR URLs:     github.com/owner/repo/pull/123
 * - Branch URLs: github.com/owner/repo/tree/branch-name
 * - Repo URLs:   github.com/owner/repo
 */
export function parseGitHubUrl(input: string): RepoSelection | null {
  const trimmed = input.trim();

  // Match PR URLs: github.com/owner/repo/pull/123
  const prMatch = trimmed.match(
    /^(?:https?:\/\/)?github\.com\/([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+?)\/pull\/(\d+)(?:\/.*)?$/
  );
  if (prMatch) {
    return { nameWithOwner: prMatch[1], prNumber: parseInt(prMatch[2], 10) };
  }

  // Match branch/tree URLs: github.com/owner/repo/tree/branch-name
  // Branch names can contain slashes, so capture everything after /tree/
  const treeMatch = trimmed.match(
    /^(?:https?:\/\/)?github\.com\/([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+?)\/tree\/(.+?)(?:[?#].*)?$/
  );
  if (treeMatch) {
    return { nameWithOwner: treeMatch[1], branchName: treeMatch[2] };
  }

  // Plain repo URL: github.com/owner/repo
  const repoMatch = trimmed.match(
    /^(?:https?:\/\/)?github\.com\/([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+?)(?:\/.*|\.git)?$/
  );
  if (repoMatch) {
    return { nameWithOwner: repoMatch[1] };
  }

  return null;
}
