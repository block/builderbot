/**
 * Repository State Store
 *
 * Manages the current repository path and recent repositories list.
 * Persists recent repos to localStorage.
 */

import { open } from '@tauri-apps/plugin-dialog';
import { getRepoInfo } from '../services/git';

// =============================================================================
// Constants
// =============================================================================

const RECENT_REPOS_KEY = 'staged-recent-repos';
const MAX_RECENT_REPOS = 10;

// =============================================================================
// Types
// =============================================================================

export interface RepoEntry {
  path: string;
  name: string;
}

// =============================================================================
// Reactive State
// =============================================================================

export const repoState = $state({
  /** Current repository path, null if none loaded */
  currentPath: null as string | null,
  /** Display name for current repo */
  currentName: 'No Repository',
  /** Whether we're in an error state (e.g., not a git repo) */
  error: null as string | null,
  /** List of recent repositories */
  recentRepos: [] as RepoEntry[],
});

// =============================================================================
// Persistence
// =============================================================================

function loadRecentRepos(): RepoEntry[] {
  try {
    const saved = localStorage.getItem(RECENT_REPOS_KEY);
    if (saved) {
      const parsed = JSON.parse(saved);
      if (Array.isArray(parsed)) {
        return parsed.slice(0, MAX_RECENT_REPOS);
      }
    }
  } catch {
    // Ignore parse errors
  }
  return [];
}

function saveRecentRepos(repos: RepoEntry[]): void {
  localStorage.setItem(RECENT_REPOS_KEY, JSON.stringify(repos.slice(0, MAX_RECENT_REPOS)));
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
 * Initialize repo state - load recent repos and try to open current directory.
 * Returns true if a repo was successfully loaded.
 */
export async function initRepoState(): Promise<boolean> {
  repoState.recentRepos = loadRecentRepos();

  // Try current directory first
  try {
    const info = await getRepoInfo();
    if (info?.repo_path) {
      repoState.currentPath = info.repo_path;
      repoState.currentName = extractRepoName(info.repo_path);
      repoState.error = null;
      addToRecentRepos({ path: info.repo_path, name: repoState.currentName });
      return true;
    }
  } catch {
    // Not in a git repo - that's fine
  }

  // No repo loaded
  repoState.currentPath = null;
  repoState.currentName = 'No Repository';
  repoState.error = null;
  return false;
}

/**
 * Open a repository by path.
 * Returns true if successful, false if not a git repo.
 */
export async function openRepo(path: string): Promise<boolean> {
  try {
    const info = await getRepoInfo(path);
    if (info?.repo_path) {
      repoState.currentPath = info.repo_path;
      repoState.currentName = extractRepoName(info.repo_path);
      repoState.error = null;
      addToRecentRepos({ path: info.repo_path, name: repoState.currentName });
      return true;
    }
  } catch (e) {
    repoState.currentPath = path;
    repoState.currentName = extractRepoName(path);
    repoState.error = e instanceof Error ? e.message : String(e);
    return false;
  }

  repoState.error = 'Not a git repository';
  return false;
}

/**
 * Open a directory picker and try to open the selected directory as a repo.
 * Returns true if a repo was successfully opened.
 */
export async function openRepoPicker(): Promise<boolean> {
  const selected = await open({
    directory: true,
    multiple: false,
    title: 'Select Repository',
  });

  if (selected && typeof selected === 'string') {
    return openRepo(selected);
  }

  return false;
}

/**
 * Remove a repo from the recent list.
 */
export function removeFromRecent(path: string): void {
  repoState.recentRepos = repoState.recentRepos.filter((r) => r.path !== path);
  saveRecentRepos(repoState.recentRepos);
}
