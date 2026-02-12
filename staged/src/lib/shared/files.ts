/**
 * File and directory browsing services.
 *
 * Wraps Tauri commands for directory listing, repo search,
 * and Spotlight-based recent repo discovery.
 */

import { invoke } from '@tauri-apps/api/core';

/** Entry in a directory listing. */
export interface DirEntry {
  name: string;
  path: string;
  isDir: boolean;
  isRepo: boolean;
}

/** A recently active git repository (from Spotlight). */
export interface RecentRepo {
  name: string;
  path: string;
}

/** List contents of a directory. Hidden files excluded. */
export function listDirectory(path: string): Promise<DirEntry[]> {
  return invoke<DirEntry[]>('list_directory', { path });
}

/**
 * Search for git repos matching a query, recursively.
 * When at home dir, only searches common dev folders.
 */
export function searchDirectories(
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

/** Get the user's home directory path. */
export function getHomeDir(): Promise<string> {
  return invoke<string>('get_home_dir');
}

/** Find git repos recently active via macOS Spotlight. */
export function findRecentRepos(hoursAgo?: number, limit?: number): Promise<RecentRepo[]> {
  return invoke<RecentRepo[]>('find_recent_repos', { hoursAgo, limit });
}
