import { invoke } from '@tauri-apps/api/core';
import type { RepoInfo, GitRef, FileDiff } from '../types';

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

// =============================================================================
// Diff API
// =============================================================================

/**
 * Get the full diff between two refs.
 * Returns all changed files with their content and alignments.
 */
export async function getDiff(base: string, head: string, repoPath?: string): Promise<FileDiff[]> {
  return invoke<FileDiff[]>('get_diff', {
    repoPath: repoPath ?? null,
    base,
    head,
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
