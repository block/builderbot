import { invoke } from '@tauri-apps/api/core';
import type {
  DiffSpec,
  FileDiffSummary,
  FileDiff,
  PullRequest,
  GitHubAuthStatus,
  GitHubSyncResult,
} from '../types';

// =============================================================================
// Git Commands
// =============================================================================

/**
 * Get the absolute path to the repository root.
 */
export async function getRepoRoot(repoPath?: string): Promise<string> {
  return invoke<string>('get_repo_root', {
    repoPath: repoPath ?? null,
  });
}

/**
 * List refs (branches, tags, remotes) for autocomplete.
 */
export async function listRefs(repoPath?: string): Promise<string[]> {
  return invoke<string[]>('list_refs', {
    repoPath: repoPath ?? null,
  });
}

/**
 * Resolve a ref to its full SHA. Used for validation.
 */
export async function resolveRef(reference: string, repoPath?: string): Promise<string> {
  return invoke<string>('resolve_ref', {
    repoPath: repoPath ?? null,
    reference,
  });
}

/**
 * Compute the merge-base between two refs.
 * Returns the SHA of the common ancestor.
 */
export async function getMergeBase(ref1: string, ref2: string, repoPath?: string): Promise<string> {
  return invoke<string>('get_merge_base', {
    repoPath: repoPath ?? null,
    ref1,
    ref2,
  });
}

/**
 * List files changed in a diff (for sidebar).
 */
export async function listDiffFiles(spec: DiffSpec, repoPath?: string): Promise<FileDiffSummary[]> {
  return invoke<FileDiffSummary[]>('list_diff_files', {
    repoPath: repoPath ?? null,
    spec,
  });
}

/**
 * Get full diff content for a single file.
 */
export async function getFileDiff(
  spec: DiffSpec,
  filePath: string,
  repoPath?: string
): Promise<FileDiff> {
  return invoke<FileDiff>('get_file_diff', {
    repoPath: repoPath ?? null,
    spec,
    filePath,
  });
}

/**
 * Create a commit with the specified files.
 * Returns the short SHA of the new commit.
 */
export async function commit(paths: string[], message: string, repoPath?: string): Promise<string> {
  return invoke<string>('commit', {
    repoPath: repoPath ?? null,
    paths,
    message,
  });
}

// =============================================================================
// GitHub Commands
// =============================================================================

/**
 * Check if GitHub CLI is installed and authenticated.
 */
export async function checkGitHubAuth(): Promise<GitHubAuthStatus> {
  return invoke<GitHubAuthStatus>('check_github_auth');
}

/**
 * List open pull requests for the repo.
 */
export async function listPullRequests(repoPath?: string): Promise<PullRequest[]> {
  return invoke<PullRequest[]>('list_pull_requests', {
    repoPath: repoPath ?? null,
  });
}

/**
 * Search for pull requests on GitHub using a query string.
 * Uses GitHub's search syntax.
 */
export async function searchPullRequests(query: string, repoPath?: string): Promise<PullRequest[]> {
  return invoke<PullRequest[]>('search_pull_requests', {
    repoPath: repoPath ?? null,
    query,
  });
}

/**
 * Invalidate the PR list cache, forcing a fresh fetch on next request.
 */
export async function invalidatePRCache(repoPath?: string): Promise<void> {
  return invoke<void>('invalidate_pr_cache', {
    repoPath: repoPath ?? null,
  });
}

/**
 * Fetch PR refs and compute merge-base.
 * Returns DiffSpec with concrete SHAs.
 */
export async function fetchPR(
  baseRef: string,
  prNumber: number,
  repoPath?: string
): Promise<DiffSpec> {
  return invoke<DiffSpec>('fetch_pr', {
    repoPath: repoPath ?? null,
    baseRef,
    prNumber,
  });
}

/**
 * Sync local review comments to a GitHub PR as a pending review.
 * Deletes any existing pending review and creates a new one.
 * Returns the URL to the pending review on GitHub.
 */
export async function syncReviewToGitHub(
  prNumber: number,
  spec: DiffSpec,
  repoPath?: string
): Promise<GitHubSyncResult> {
  return invoke<GitHubSyncResult>('sync_review_to_github', {
    repoPath: repoPath ?? null,
    prNumber,
    spec,
  });
}
