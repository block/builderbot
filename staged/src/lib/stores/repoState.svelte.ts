/**
 * Repository State Store
 *
 * Manages the current repository path and recent repositories list.
 * Persists recent repos to Tauri store (not localStorage, which breaks on dev port changes).
 */

import { getRepoRoot } from '../services/git';
import { getInitialPath } from '../services/window';
import {
  loadRecentReposFromStore,
  saveRecentReposToStore,
  type RepoEntry,
} from './preferences.svelte';

// Re-export the type
export type { RepoEntry };

// =============================================================================
// Constants
// =============================================================================

const MAX_RECENT_REPOS = 10;

// =============================================================================
// Reactive State
// =============================================================================

export const repoState = $state({
  /** Current repository path, null if none loaded */
  currentPath: null as string | null,
  /** Display name for current repo */
  currentName: 'No Repository',
  /** List of recent repositories */
  recentRepos: [] as RepoEntry[],
});

// =============================================================================
// Persistence
// =============================================================================

async function loadRecentRepos(): Promise<RepoEntry[]> {
  const repos = await loadRecentReposFromStore();
  return repos.slice(0, MAX_RECENT_REPOS);
}

function saveRecentRepos(repos: RepoEntry[]): void {
  // Fire and forget - don't block on save
  saveRecentReposToStore(repos.slice(0, MAX_RECENT_REPOS));
}

function addToRecentRepos(entry: RepoEntry): void {
  // Remove if already exists, then add to front
  const filtered = repoState.recentRepos.filter((r) => r.path !== entry.path);
  repoState.recentRepos = [entry, ...filtered].slice(0, MAX_RECENT_REPOS);
  saveRecentRepos(repoState.recentRepos);
}

// =============================================================================
// Helpers
// =============================================================================

/**
 * Extract repo name from path (last directory component)
 */
function extractRepoName(repoPath: string): string {
  const cleanPath = repoPath.replace(/\/$/, '');
  const parts = cleanPath.split('/');
  return parts[parts.length - 1] || 'Repository';
}

// =============================================================================
// Actions
// =============================================================================

/**
 * Initialize repo state - load recent repos and resolve initial path.
 * Priority: CLI argument > current directory
 * Returns the canonical repo path, or null if not in a git repo.
 */
export async function initRepoState(): Promise<string | null> {
  repoState.recentRepos = await loadRecentRepos();

  // Check for CLI argument first (e.g., `staged /path/to/repo`)
  let initialPath = await getInitialPath();

  // Fall back to current directory if no CLI argument
  if (!initialPath) {
    initialPath = '.';
  }

  // Resolve to canonical path and validate it's a git repo
  try {
    const canonicalPath = await getRepoRoot(initialPath);

    // Check if this path is already in recent repos
    const existing = repoState.recentRepos.find((r) => r.path === canonicalPath);
    if (existing) {
      // Reuse existing entry (moves it to front)
      repoState.currentPath = existing.path;
      repoState.currentName = existing.name;
      addToRecentRepos(existing);
    } else {
      // New repo - add it
      repoState.currentPath = canonicalPath;
      repoState.currentName = extractRepoName(canonicalPath);
      addToRecentRepos({ path: canonicalPath, name: repoState.currentName });
    }
    return canonicalPath;
  } catch {
    // Not in a git repo - leave state as null
    repoState.currentPath = null;
    repoState.currentName = 'No Repository';
    return null;
  }
}

/**
 * Set the current repository after successful operations.
 * Call this after confirming a path works (e.g., after loading refs/diffs).
 */
export function setCurrentRepo(path: string): void {
  repoState.currentPath = path;
  repoState.currentName = extractRepoName(path);
  addToRecentRepos({ path, name: repoState.currentName });
}

/**
 * Open a repository by path.
 */
export function openRepo(path: string): void {
  repoState.currentPath = path;
  repoState.currentName = extractRepoName(path);
  addToRecentRepos({ path, name: repoState.currentName });
}

/**
 * Get recent repos for display in the folder picker modal.
 */
export function getRecentRepos(): RepoEntry[] {
  return repoState.recentRepos;
}

/**
 * Remove a repo from the recent list.
 */
export function removeFromRecent(path: string): void {
  repoState.recentRepos = repoState.recentRepos.filter((r) => r.path !== path);
  saveRecentRepos(repoState.recentRepos);
}
