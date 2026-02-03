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

// Re-export types for convenience
export type { DiffState, CommentsState, DiffSelection, AgentState, ReferenceFilesState };

/**
 * State for a single tab
 */
export interface TabState {
  /** Unique identifier (repo path) */
  id: string;
  /** Full path to repository */
  repoPath: string;
  /** Display name of repository */
  repoName: string;

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
 * If the repo is already open, switches to that tab instead.
 */
export function addTab(
  repoPath: string,
  repoName: string,
  createDiffState: () => DiffState,
  createCommentsState: () => CommentsState,
  createDiffSelection: () => DiffSelection,
  createAgentState: () => AgentState,
  createReferenceFilesState: () => ReferenceFilesState
): void {
  // Check if tab already exists
  const existingIndex = windowState.tabs.findIndex((t) => t.id === repoPath);
  if (existingIndex !== -1) {
    // Switch to existing tab
    windowState.activeTabIndex = existingIndex;
    return;
  }

  // Create new tab with isolated state instances
  // Plain objects are created - the parent windowState.tabs array is already reactive
  const tab: TabState = {
    id: repoPath,
    repoPath,
    repoName,
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

  // Stop watching if no other tab uses this repo
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

const STORAGE_KEY_PREFIX = 'staged-window-';

/**
 * Save tabs to localStorage.
 */
function saveTabsToStorage(): void {
  const key = `${STORAGE_KEY_PREFIX}${windowState.windowLabel}-tabs`;
  const data = {
    tabs: windowState.tabs.map((t) => ({
      id: t.id,
      repoPath: t.repoPath,
      repoName: t.repoName,
    })),
    activeTabIndex: windowState.activeTabIndex,
  };
  localStorage.setItem(key, JSON.stringify(data));
}

/**
 * Load tabs from localStorage.
 * Tabs are recreated with fresh state instances.
 */
export function loadTabsFromStorage(
  createDiffState: () => DiffState,
  createCommentsState: () => CommentsState,
  createDiffSelection: () => DiffSelection,
  createAgentState: () => AgentState,
  createReferenceFilesState: () => ReferenceFilesState
): void {
  const key = `${STORAGE_KEY_PREFIX}${windowState.windowLabel}-tabs`;
  const stored = localStorage.getItem(key);

  if (stored) {
    try {
      const data = JSON.parse(stored);
      // Create tabs with isolated state instances
      // Plain objects are created - the parent windowState.tabs array is already reactive
      windowState.tabs = data.tabs.map((t: any) => ({
        id: t.id,
        repoPath: t.repoPath,
        repoName: t.repoName,
        diffState: createDiffState(),
        commentsState: createCommentsState(),
        diffSelection: createDiffSelection(),
        agentState: createAgentState(),
        referenceFilesState: createReferenceFilesState(),
        needsRefresh: false,
      }));
      windowState.activeTabIndex = data.activeTabIndex;
    } catch (e) {
      console.error('Failed to load tabs from storage:', e);
    }
  }
}

/**
 * Set the window label (called on app mount).
 */
export function setWindowLabel(label: string): void {
  windowState.windowLabel = label;
}
