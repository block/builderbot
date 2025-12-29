import { invoke } from '@tauri-apps/api/core';
import type { GitStatus, FileDiff, CommitResult, ChangedFile, GitRef } from '../types';

export async function getGitStatus(path?: string): Promise<GitStatus> {
  return invoke<GitStatus>('get_git_status', { path: path ?? null });
}

/**
 * Get list of files changed between two refs.
 * Used to populate the sidebar when viewing a diff.
 */
export async function getChangedFiles(
  base: string,
  head: string,
  repoPath?: string
): Promise<ChangedFile[]> {
  return invoke<ChangedFile[]>('get_changed_files', {
    repoPath: repoPath ?? null,
    base,
    head,
  });
}

export async function openRepository(path: string): Promise<GitStatus> {
  return invoke<GitStatus>('open_repository', { path });
}

/**
 * Get diff for a file between two refs.
 * This is the primary diff function for the review model.
 *
 * @param base - Base ref (branch name, SHA, "HEAD", etc.)
 * @param head - Head ref (same as base, or "@" for working tree)
 * @param filePath - Path to file relative to repo root
 * @param repoPath - Optional path to repository
 */
export async function getRefDiff(
  base: string,
  head: string,
  filePath: string,
  repoPath?: string
): Promise<FileDiff> {
  return invoke<FileDiff>('get_ref_diff', {
    repoPath: repoPath ?? null,
    base,
    head,
    filePath,
  });
}

// Staging operations

export async function stageFile(filePath: string, repoPath?: string): Promise<void> {
  return invoke('stage_file', {
    repoPath: repoPath ?? null,
    filePath,
  });
}

export async function unstageFile(filePath: string, repoPath?: string): Promise<void> {
  return invoke('unstage_file', {
    repoPath: repoPath ?? null,
    filePath,
  });
}

export async function discardFile(filePath: string, repoPath?: string): Promise<void> {
  return invoke('discard_file', {
    repoPath: repoPath ?? null,
    filePath,
  });
}

export async function stageAll(repoPath?: string): Promise<void> {
  return invoke('stage_all', {
    repoPath: repoPath ?? null,
  });
}

export async function unstageAll(repoPath?: string): Promise<void> {
  return invoke('unstage_all', {
    repoPath: repoPath ?? null,
  });
}

// Commit operations

export async function getLastCommitMessage(repoPath?: string): Promise<string | null> {
  return invoke<string | null>('get_last_commit_message', {
    repoPath: repoPath ?? null,
  });
}

export async function createCommit(message: string, repoPath?: string): Promise<CommitResult> {
  return invoke<CommitResult>('create_commit', {
    repoPath: repoPath ?? null,
    message,
  });
}

export async function amendCommit(message: string, repoPath?: string): Promise<CommitResult> {
  return invoke<CommitResult>('amend_commit', {
    repoPath: repoPath ?? null,
    message,
  });
}

// Line-level operations

import type { SourceLines } from '../types';

export async function discardLines(
  filePath: string,
  sourceLines: SourceLines,
  staged: boolean,
  repoPath?: string
): Promise<void> {
  return invoke('discard_lines', {
    repoPath: repoPath ?? null,
    filePath,
    oldStart: sourceLines.old_start,
    oldEnd: sourceLines.old_end,
    newStart: sourceLines.new_start,
    newEnd: sourceLines.new_end,
    staged,
  });
}

// Ref operations (for autocomplete and display)

/**
 * Get list of refs (branches, tags, special refs) for autocomplete.
 */
export async function getRefs(repoPath?: string): Promise<GitRef[]> {
  return invoke<GitRef[]>('get_refs', {
    repoPath: repoPath ?? null,
  });
}

/**
 * Resolve a ref to its short SHA for display.
 */
export async function resolveRef(refStr: string, repoPath?: string): Promise<string> {
  return invoke<string>('resolve_ref', {
    repoPath: repoPath ?? null,
    refStr,
  });
}
