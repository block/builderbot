import { invoke } from '@tauri-apps/api/core';
import type { GitStatus, FileDiff, CommitResult } from '../types';

export async function getGitStatus(path?: string): Promise<GitStatus> {
  return invoke<GitStatus>('get_git_status', { path: path ?? null });
}

export async function openRepository(path: string): Promise<GitStatus> {
  return invoke<GitStatus>('open_repository', { path });
}

export async function getFileDiff(
  filePath: string,
  staged: boolean,
  repoPath?: string
): Promise<FileDiff> {
  return invoke<FileDiff>('get_file_diff', {
    repoPath: repoPath ?? null,
    filePath,
    staged,
  });
}

export async function getUntrackedFileDiff(filePath: string, repoPath?: string): Promise<FileDiff> {
  return invoke<FileDiff>('get_untracked_file_diff', {
    repoPath: repoPath ?? null,
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
