import { invoke } from '@tauri-apps/api/core';
import type {
  RepoInfo,
  GitRef,
  FileDiff,
  PullRequest,
  GitHubAuthStatus,
  PRFetchResult,
} from '../types';

// =============================================================================
// Repository Info
// =============================================================================

/**
 * Get basic repository info (path and branch name).
 */
export async function getRepoInfo(repoPath?: string): Promise<RepoInfo> {
  return invoke<RepoInfo>('get_repo_info', {
    repoPath: repoPath ?? null,
  });
}

/**
 * Get the last commit message (for amend UI).
 */
export async function getLastCommitMessage(repoPath?: string): Promise<string | null> {
  return invoke<string | null>('get_last_commit_message', {
    repoPath: repoPath ?? null,
  });
}

/**
 * Create a commit with the specified files and message.
 * Returns the short SHA of the new commit.
 */
export async function createCommit(
  paths: string[],
  message: string,
  repoPath?: string
): Promise<string> {
  return invoke<string>('create_commit', {
    repoPath: repoPath ?? null,
    paths,
    message,
  });
}

// =============================================================================
// Diff API
// =============================================================================

/**
 * Get the full diff between two refs.
 * If `useMergeBase` is true, diffs from the merge-base instead of base directly.
 */
export async function getDiff(
  base: string,
  head: string,
  repoPath?: string,
  useMergeBase?: boolean
): Promise<FileDiff[]> {
  return invoke<FileDiff[]>('get_diff', {
    repoPath: repoPath ?? null,
    base,
    head,
    useMergeBase: useMergeBase ?? false,
  });
}

/**
 * Get list of refs (branches, tags, special refs) with type info for autocomplete.
 */
export async function getRefs(repoPath?: string): Promise<GitRef[]> {
  return invoke<GitRef[]>('get_refs', {
    repoPath: repoPath ?? null,
  });
}

/**
 * Resolve a ref to its short SHA for display/validation.
 * Returns "working tree" for "WORKDIR", otherwise returns short SHA.
 */
export async function resolveRef(refStr: string, repoPath?: string): Promise<string> {
  return invoke<string>('resolve_ref', {
    repoPath: repoPath ?? null,
    refStr,
  });
}

// =============================================================================
// GitHub API
// =============================================================================

/**
 * Check if the user is authenticated with GitHub CLI.
 */
export async function checkGitHubAuth(): Promise<GitHubAuthStatus> {
  return invoke<GitHubAuthStatus>('check_github_auth');
}

/**
 * List open pull requests for the current repository.
 * Uses caching by default; pass forceRefresh=true to bypass.
 */
export async function listPullRequests(
  repoPath?: string,
  forceRefresh?: boolean
): Promise<PullRequest[]> {
  return invoke<PullRequest[]>('list_pull_requests', {
    repoPath: repoPath ?? null,
    forceRefresh: forceRefresh ?? false,
  });
}

/**
 * Fetch a PR branch from the remote and set up locally.
 * This is idempotent - if the branch already exists, it will be updated.
 * Returns both the merge-base SHA and head SHA for stable diff identification.
 */
export async function fetchPRBranch(
  baseRef: string,
  prNumber: number,
  repoPath?: string
): Promise<PRFetchResult> {
  return invoke<PRFetchResult>('fetch_pr_branch', {
    repoPath: repoPath ?? null,
    baseRef,
    prNumber,
  });
}
