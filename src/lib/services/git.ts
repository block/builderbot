import { invoke } from '@tauri-apps/api/core';
import type { GitStatus } from '../types';

export async function getGitStatus(path?: string): Promise<GitStatus> {
  return invoke<GitStatus>('get_git_status', { path: path ?? null });
}

export async function openRepository(path: string): Promise<GitStatus> {
  return invoke<GitStatus>('open_repository', { path });
}
