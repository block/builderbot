import { invoke } from '@tauri-apps/api/core';
import type { File } from '../types';

// =============================================================================
// Directory Browsing API
// =============================================================================

/**
 * Entry in a directory listing.
 */
export interface DirEntry {
  name: string;
  path: string;
  isDir: boolean;
  isRepo: boolean;
}

/**
 * List contents of a directory.
 * Returns directories first (sorted), then files (sorted).
 * Hidden files (starting with .) are excluded.
 */
export async function listDirectory(path: string): Promise<DirEntry[]> {
  return invoke<DirEntry[]>('list_directory', { path });
}

/**
 * Search for directories matching a query, recursively.
 * Uses prefix/substring matching. Skips system folders and prioritizes dev locations.
 * Returns up to `limit` matches sorted by relevance.
 */
export async function searchDirectories(
  path: string,
  query: string,
  maxDepth?: number,
  limit?: number
): Promise<DirEntry[]> {
  return invoke<DirEntry[]>('search_directories', {
    path,
    query,
    maxDepth: maxDepth ?? 3,
    limit: limit ?? 20,
  });
}

/**
 * Get the user's home directory path.
 */
export async function getHomeDir(): Promise<string> {
  return invoke<string>('get_home_dir');
}

// =============================================================================
// Recent Repos API
// =============================================================================

/**
 * A recently active git repository.
 */
export interface RecentRepo {
  name: string;
  path: string;
}

/**
 * Find git repositories that have been recently active.
 * Uses macOS Spotlight to find files modified within the last `hoursAgo` hours.
 */
export async function findRecentRepos(hoursAgo?: number, limit?: number): Promise<RecentRepo[]> {
  return invoke<RecentRepo[]>('find_recent_repos', { hoursAgo, limit });
}

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
