import { invoke } from '@tauri-apps/api/core';
import type { File } from '../types';

// =============================================================================
// File Browsing API
// =============================================================================

/**
 * Search for files matching a query in the repository.
 *
 * Uses fuzzy matching - returns up to `limit` matches sorted by relevance.
 * Matches if all query characters appear in order in the file path (case-insensitive).
 */
export async function searchFiles(
  refName: string,
  query: string,
  limit?: number,
  repoPath?: string
): Promise<string[]> {
  return invoke<string[]>('search_files', {
    repoPath: repoPath ?? null,
    refName,
    query,
    limit: limit ?? 20,
  });
}

/**
 * Get the content of a file at a specific ref.
 *
 * For WORKDIR, reads from the working directory.
 * For other refs, reads from the git tree.
 */
export async function getFileAtRef(
  refName: string,
  path: string,
  repoPath?: string
): Promise<File> {
  return invoke<File>('get_file_at_ref', {
    repoPath: repoPath ?? null,
    refName,
    path,
  });
}
