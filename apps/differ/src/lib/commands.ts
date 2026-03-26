/**
 * Typed invoke wrappers for Differ's Tauri commands.
 */

import { invoke } from '@tauri-apps/api/core';
import type { FileDiffSummary, FileDiff, File } from '@builderbot/diff-viewer/types';

// =============================================================================
// Types (matching Rust backend)
// =============================================================================

export interface RepoInfo {
  path: string;
  branch: string;
  defaultBranch: string;
  commitsAhead: number;
}

export interface CommitInfo {
  sha: string;
  shortSha: string;
  message: string;
  author: string;
  timestamp: number;
}

export interface DiffFilesResponse {
  files: FileDiffSummary[];
}

export interface LaunchArgs {
  repoPath: string;
  mode: string | null;
  commit: string | null;
}

export interface DirEntry {
  name: string;
  path: string;
  isDir: boolean;
  isRepo: boolean;
}

export interface RecentRepo {
  name: string;
  path: string;
}

export interface StackBranchInfo {
  name: string;
  parentRef: string;
}

export interface StackInfo {
  currentBranch: string;
  parentBranch: string;
  parentSha: string;
  isTrunkChild: boolean;
  needsRestack: boolean;
  downstack: StackBranchInfo[];
  upstack: StackBranchInfo[];
}

/** Matches the git-diff crate's GitRef enum (tagged union). */
export type GitRef =
  | { type: 'WorkingTree' }
  | { type: 'Index' }
  | { type: 'Rev'; value: string }
  | { type: 'MergeBase' }
  | { type: 'MergeBaseOf'; value: [string, string] };

export interface DiffSpec {
  base: GitRef;
  head: GitRef;
}

// =============================================================================
// Diff spec builders
// =============================================================================

/** All uncommitted changes: HEAD -> working tree */
export function specUncommitted(): DiffSpec {
  return {
    base: { type: 'Rev', value: 'HEAD' },
    head: { type: 'WorkingTree' },
  };
}

/** Staged changes only: HEAD -> index */
export function specStaged(): DiffSpec {
  return {
    base: { type: 'Rev', value: 'HEAD' },
    head: { type: 'Index' },
  };
}

/** Full branch diff: merge-base -> HEAD */
export function specBranch(): DiffSpec {
  return {
    base: { type: 'MergeBase' },
    head: { type: 'Rev', value: 'HEAD' },
  };
}

/** Single commit: parent -> commit */
export function specCommit(sha: string): DiffSpec {
  return {
    base: { type: 'Rev', value: `${sha}~1` },
    head: { type: 'Rev', value: sha },
  };
}

/** Range from a commit to HEAD */
export function specRange(fromSha: string): DiffSpec {
  return {
    base: { type: 'Rev', value: fromSha },
    head: { type: 'Rev', value: 'HEAD' },
  };
}

/** This PR's diff: parent branch → working tree (includes uncommitted) */
export function specStack(parentBranch: string): DiffSpec {
  return {
    base: { type: 'MergeBaseOf', value: [parentBranch, 'HEAD'] },
    head: { type: 'WorkingTree' },
  };
}

/** This PR's committed diff only: parent branch → HEAD */
export function specStackCommitted(parentBranch: string): DiffSpec {
  return {
    base: { type: 'MergeBaseOf', value: [parentBranch, 'HEAD'] },
    head: { type: 'Rev', value: 'HEAD' },
  };
}

/** View another branch in the stack (read-only, no checkout needed) */
export function specStackBranch(branchName: string, parentBranch: string): DiffSpec {
  return {
    base: { type: 'MergeBaseOf', value: [parentBranch, branchName] },
    head: { type: 'Rev', value: branchName },
  };
}

// =============================================================================
// Commands
// =============================================================================

export function getRepoInfo(): Promise<RepoInfo> {
  return invoke('get_repo_info');
}

export function getStackInfo(): Promise<StackInfo | null> {
  return invoke('get_stack_info');
}

export function listRecentCommits(count?: number): Promise<CommitInfo[]> {
  return invoke('list_recent_commits', { count: count ?? null });
}

export function listDiffFiles(spec: DiffSpec): Promise<DiffFilesResponse> {
  return invoke('list_diff_files', { spec });
}

export function getFileDiff(spec: DiffSpec, path: string): Promise<FileDiff> {
  return invoke('get_file_diff', { spec, path });
}

export function getFileAtRef(refName: string, path: string): Promise<File> {
  return invoke('get_file_at_ref', { refName, path });
}

export function getLaunchArgs(): Promise<LaunchArgs> {
  return invoke('get_launch_args');
}

export function setRepoPath(path: string): Promise<void> {
  return invoke('set_repo_path', { path });
}

// =============================================================================
// Directory browsing
// =============================================================================

export function listDirectory(path: string): Promise<DirEntry[]> {
  return invoke('list_directory', { path });
}

export function searchDirectories(
  path: string,
  query: string,
  maxDepth?: number,
  limit?: number
): Promise<DirEntry[]> {
  return invoke('search_directories', {
    path,
    query,
    maxDepth: maxDepth ?? 3,
    limit: limit ?? 20,
  });
}

export function getHomeDir(): Promise<string> {
  return invoke('get_home_dir');
}

export function findRecentRepos(hoursAgo?: number, limit?: number): Promise<RecentRepo[]> {
  return invoke('find_recent_repos', { hoursAgo, limit });
}

export function listSystemFonts(): Promise<string[]> {
  return invoke('list_system_fonts');
}

export interface CustomTheme {
  name: string;
  json: string;
}

export function listCustomThemes(): Promise<CustomTheme[]> {
  return invoke('list_custom_themes');
}
