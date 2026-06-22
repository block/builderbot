/**
 * Synchronous, web-only boot snapshots.
 *
 * On iOS, Safari tears down the tab and reloads the page from scratch when the
 * user leaves and returns. Both the Tauri-style persistent store and the
 * IndexedDB SWR cache are read asynchronously, so neither can paint on the very
 * first frame of a cold boot. These helpers persist tiny snapshots to
 * `localStorage` (a synchronous API) so the navigation route and cached branch
 * timelines can be restored before any component mounts.
 *
 * No-ops in Tauri mode (the native app is never torn down this way) and tolerant
 * of environments where `localStorage` is unavailable or full.
 */

import { isTauri } from '../transport';

/** localStorage keys for the synchronous boot accelerators. */
export const SNAPSHOT_KEYS = {
  /** The last viewed project id, mirrored for synchronous restore. */
  lastProject: 'staged:boot:last-project',
  /** Compact snapshot of the in-memory branch timeline cache. */
  timelines: 'staged:boot:timelines',
} as const;

export function readSnapshot<T>(key: string): T | null {
  if (isTauri || typeof localStorage === 'undefined') return null;
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

export function writeSnapshot<T>(key: string, value: T): void {
  if (isTauri || typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Best-effort — ignore quota or serialization errors.
  }
}

export function clearSnapshot(key: string): void {
  if (isTauri || typeof localStorage === 'undefined') return;
  try {
    localStorage.removeItem(key);
  } catch {
    // Best-effort — ignore.
  }
}
