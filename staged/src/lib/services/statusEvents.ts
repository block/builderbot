/**
 * File watcher event subscription service.
 *
 * Manages multiple watchers (one per repo). Each repo gets a unique watch ID
 * so the frontend can identify which repo changed. Watchers are only stopped
 * when explicitly unwatched (e.g., when closing a tab with no other tabs using that repo).
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';

/** Callback for file change notifications */
export type FilesChangedCallback = (repoPath: string) => void;

/** Cleanup function returned by subscribe */
export type Unsubscribe = () => void;

/** Payload from backend files-changed event */
interface FilesChangedPayload {
  watchId: number;
}

// Track watched repos: path -> watchId
const watchedRepos = new Map<string, number>();

// Reverse lookup: watchId -> path
const watchIdToPath = new Map<number, string>();

// Next watch ID to assign
let nextWatchId = 0;

// Active listener
let filesChangedUnlisten: UnlistenFn | null = null;

/**
 * Initialize the watcher event listener.
 * Call once on app startup. The callback is invoked when files change
 * in any watched repo, with the repo path as argument.
 *
 * @param onFilesChanged - Called when files in a watched repo change
 * @returns Cleanup function to unsubscribe
 */
export async function initWatcher(onFilesChanged: FilesChangedCallback): Promise<Unsubscribe> {
  // Clean up any existing listener
  if (filesChangedUnlisten) {
    filesChangedUnlisten();
  }

  filesChangedUnlisten = await listen<FilesChangedPayload>('files-changed', ({ payload }) => {
    // Look up which repo this event is for
    const repoPath = watchIdToPath.get(payload.watchId);
    if (repoPath) {
      onFilesChanged(repoPath);
    } else {
      console.warn(
        `[FileWatcher] Received event for unknown watchId ${payload.watchId}. ` +
          `Known watchIds: [${Array.from(watchIdToPath.keys()).join(', ')}]`
      );
    }
  });

  return () => {
    if (filesChangedUnlisten) {
      filesChangedUnlisten();
      filesChangedUnlisten = null;
    }
  };
}

/**
 * Start watching a repository (idempotent).
 * Fire-and-forget: returns immediately, actual setup happens on backend thread.
 *
 * @param repoPath - Absolute path to the repository
 */
export function watchRepo(repoPath: string): void {
  // Already watching this repo
  if (watchedRepos.has(repoPath)) {
    return;
  }

  const watchId = nextWatchId++;
  watchedRepos.set(repoPath, watchId);
  watchIdToPath.set(watchId, repoPath);

  console.debug(`[FileWatcher] Starting watch for ${repoPath} (watchId: ${watchId})`);
  invoke('watch_repo', { repoPath, watchId }).catch((err) => {
    console.error(`[FileWatcher] Failed to start watching ${repoPath}:`, err);
    // Rollback frontend state on failure
    watchedRepos.delete(repoPath);
    watchIdToPath.delete(watchId);
  });
}

/**
 * Stop watching a repository.
 * Fire-and-forget: returns immediately, actual teardown happens on backend thread.
 *
 * @param repoPath - Absolute path to the repository
 */
export function unwatchRepo(repoPath: string): void {
  const watchId = watchedRepos.get(repoPath);
  if (watchId === undefined) {
    return;
  }

  watchedRepos.delete(repoPath);
  watchIdToPath.delete(watchId);

  console.debug(`[FileWatcher] Stopping watch for ${repoPath} (watchId: ${watchId})`);
  invoke('unwatch_repo', { repoPath }).catch((err) => {
    console.error(`[FileWatcher] Failed to stop watching ${repoPath}:`, err);
  });
}

/**
 * Check if a repo is currently being watched.
 */
export function isWatching(repoPath: string): boolean {
  return watchedRepos.has(repoPath);
}

/**
 * Get the number of active watchers (for debugging).
 */
export function getWatcherCount(): number {
  return watchedRepos.size;
}
