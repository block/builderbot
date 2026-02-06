/**
 * Tab State Store
 *
 * Manages multiple repository tabs within each window.
 * Each tab maintains isolated state for its repository (diffs, comments, selection, agent chat).
 */

import { unwatchRepo } from '../services/statusEvents';
import type { DiffState } from './diffState.svelte';
import type { CommentsState } from './comments.svelte';
import type { DiffSelection } from './diffSelection.svelte';
import type { AgentState } from './agent.svelte';
import type { ReferenceFilesState } from './referenceFiles.svelte';
import {
  loadWindowTabsFromStore,
  saveWindowTabsToStore,
  type StoredTabData,
} from './preferences.svelte';

// Re-export types for convenience
export type { DiffState, CommentsState, DiffSelection, AgentState, ReferenceFilesState };

/**
 * State for a single tab
 */
export interface TabState {
  /** Unique identifier (project ID) */
  id: string;
  /** Project ID this tab belongs to */
  projectId: string;
  /** Full path to repository */
  repoPath: string;
  /** Display name of repository */
  repoName: string;
  /** Optional subpath within the repo (for monorepos) */
  subpath: string | null;

  // Isolated state instances per tab
  diffState: DiffState;
  commentsState: CommentsState;
  diffSelection: DiffSelection;
  agentState: AgentState;
  referenceFilesState: ReferenceFilesState;

  /** True if files changed while this tab was not active (needs refresh on switch) */
  needsRefresh: boolean;
}

/**
 * Window-level state (contains multiple tabs)
 */
interface WindowTabs {
  /** All tabs in this window */
  tabs: TabState[];
  /** Index of currently active tab */
  activeTabIndex: number;
  /** Window label (for persistence) */
  windowLabel: string;
}

// =============================================================================
// Reactive State
// =============================================================================

/**
 * Window state object.
 * Use this directly in components - it's reactive!
 */
export const windowState = $state<WindowTabs>({
  tabs: [],
  activeTabIndex: 0,
  windowLabel: 'main',
});

/**
 * Get the currently active tab.
 * Returns null if no tabs exist.
 */
export function getActiveTab(): TabState | null {
  return windowState.tabs[windowState.activeTabIndex] ?? null;
}

// =============================================================================
// Tab Management Functions
// =============================================================================

/**
 * Add a new tab to the window.
 * If the project is already open, switches to that tab instead.
 */
export function addTab(
  projectId: string,
  repoPath: string,
  repoName: string,
  subpath: string | null,
  createDiffState: () => DiffState,
  createCommentsState: () => CommentsState,
  createDiffSelection: () => DiffSelection,
  createAgentState: () => AgentState,
  createReferenceFilesState: () => ReferenceFilesState
): void {
  // Check if tab already exists for this project
  const existingIndex = windowState.tabs.findIndex((t) => t.projectId === projectId);
  if (existingIndex !== -1) {
    // Switch to existing tab
    windowState.activeTabIndex = existingIndex;
    return;
  }

  // Create new tab with isolated state instances
  // Plain objects are created - the parent windowState.tabs array is already reactive
  const tab: TabState = {
    id: projectId,
    projectId,
    repoPath,
    repoName,
    subpath,
    diffState: createDiffState(),
    commentsState: createCommentsState(),
    diffSelection: createDiffSelection(),
    agentState: createAgentState(),
    referenceFilesState: createReferenceFilesState(),
    needsRefresh: false,
  };

  windowState.tabs.push(tab);
  windowState.activeTabIndex = windowState.tabs.length - 1;

  saveTabsToStorage();
}

/**
 * Close a tab by ID.
 * Closes the window if it's the last tab.
 * Stops watching the repo if no other tabs use it.
 */
export function closeTab(tabId: string): void {
  const index = windowState.tabs.findIndex((t) => t.id === tabId);
  if (index === -1) return;

  const closedTab = windowState.tabs[index];
  windowState.tabs.splice(index, 1);

  // Stop watching the repo if no other tab uses this repo
  // (multiple projects might share the same repo, so check all tabs)
  if (closedTab) {
    const stillUsed = windowState.tabs.some((t) => t.repoPath === closedTab.repoPath);
    if (!stillUsed) {
      unwatchRepo(closedTab.repoPath);
    }
  }

  // Adjust active index if needed
  if (windowState.activeTabIndex >= windowState.tabs.length) {
    windowState.activeTabIndex = Math.max(0, windowState.tabs.length - 1);
  }

  saveTabsToStorage();
}

/**
 * Switch to a tab by index.
 * Watcher is already running for the repo (started when tab was created).
 */
export function switchTab(index: number): void {
  if (index < 0 || index >= windowState.tabs.length) return;

  windowState.activeTabIndex = index;
  saveTabsToStorage();
}

/**
 * Get the currently active tab's repo path.
 */
export function getActiveRepoPath(): string | null {
  return getActiveTab()?.repoPath ?? null;
}

/**
 * Mark all tabs for a repo as needing refresh.
 * Called when files change for a non-active tab.
 * Note: This marks ALL projects using the repo, since a file change
 * anywhere in the repo could affect any project (even with different subpaths).
 */
export function markRepoNeedsRefresh(repoPath: string): void {
  for (const tab of windowState.tabs) {
    if (tab.repoPath === repoPath) {
      tab.needsRefresh = true;
      console.debug(`[TabState] Marked tab "${tab.repoName}" as needing refresh`);
    }
  }
}

/**
 * Clear the needsRefresh flag for a tab.
 * Called after refreshing the tab.
 */
export function clearNeedsRefresh(tab: TabState): void {
  tab.needsRefresh = false;
}

// =============================================================================
// Persistence
// =============================================================================

/**
 * Save tabs to persistent store.
 */
function saveTabsToStorage(): void {
  const data: StoredTabData = {
    tabs: windowState.tabs.map((t) => ({
      id: t.id,
      projectId: t.projectId,
      repoPath: t.repoPath,
      repoName: t.repoName,
      subpath: t.subpath,
    })),
    activeTabIndex: windowState.activeTabIndex,
  };
  // Fire and forget - don't block on save
  saveWindowTabsToStore(windowState.windowLabel, data);
}

/**
 * Load tabs from persistent store.
 * Tabs are recreated with fresh state instances.
 */
export async function loadTabsFromStorage(
  createDiffState: () => DiffState,
  createCommentsState: () => CommentsState,
  createDiffSelection: () => DiffSelection,
  createAgentState: () => AgentState,
  createReferenceFilesState: () => ReferenceFilesState
): Promise<void> {
  const data = await loadWindowTabsFromStore(windowState.windowLabel);

  if (data) {
    // Create tabs with isolated state instances
    // Plain objects are created - the parent windowState.tabs array is already reactive
    windowState.tabs = data.tabs.map((t) => ({
      id: t.id || t.projectId, // Fallback for old format
      projectId: t.projectId || t.id, // Fallback for old format
      repoPath: t.repoPath,
      repoName: t.repoName,
      subpath: t.subpath || null,
      diffState: createDiffState(),
      commentsState: createCommentsState(),
      diffSelection: createDiffSelection(),
      agentState: createAgentState(),
      referenceFilesState: createReferenceFilesState(),
      needsRefresh: false,
    }));
    windowState.activeTabIndex = data.activeTabIndex;
  }
}

/**
 * Set the window label (called on app mount).
 */
export function setWindowLabel(label: string): void {
  windowState.windowLabel = label;
}
