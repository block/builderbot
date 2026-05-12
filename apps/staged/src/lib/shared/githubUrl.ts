export interface RepoSelection {
  nameWithOwner: string;
  subpath?: string;
  prNumber?: number;
  branchName?: string;
  /** Pre-fetched default branch to avoid a slow API call during project/repo creation. */
  defaultBranch?: string | null;
  /** For fork PRs, the head (fork) repo slug when it differs from nameWithOwner. */
  headRepo?: string | null;
  /** PR title, carried through for auto-creating project notes. */
  prTitle?: string;
  /** PR body/description, carried through for auto-creating project notes. */
  prBody?: string;
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
  // NOTE: This pattern also matches PR and tree URLs (capturing just owner/repo),
  // so it must remain last — the ordering of these three checks is load-bearing.
  const repoMatch = trimmed.match(
    /^(?:https?:\/\/)?github\.com\/([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+?)(?:\/.*|\.git)?$/
  );
  if (repoMatch) {
    return { nameWithOwner: repoMatch[1] };
  }

  return null;
}
