/**
 * Typed invoke wrappers for Staged's Tauri commands.
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

/** Matches the git-diff crate's GitRef enum (tagged union). */
export type GitRef =
  | { type: 'WorkingTree' }
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

// =============================================================================
// Commands
// =============================================================================

export function getRepoInfo(): Promise<RepoInfo> {
  return invoke('get_repo_info');
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

export function openRepoDialog(): Promise<string | null> {
  return invoke('open_repo_dialog');
}
