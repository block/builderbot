/**
 * Global project state store
 * Tracks which projects have running sessions and unread status
 *
 * A project is marked as "unread" when:
 * - A session completes in that project while the user is viewing a different project
 *
 * A project is marked as "read" when:
 * - The user navigates to that project
 *
 * Running sessions are tracked to show spinners on project cards
 * Unread state is persisted to disk and the macOS dock badge is kept in sync.
 *
 * Note: Session-to-project lookups are now delegated to the unified sessionRegistry
 */

import { getCurrentWindow } from '@tauri-apps/api/window';
import { getStoreValue, setStoreValue } from '../shared/persistentStore';
import { sessionRegistry, type SessionType } from './sessionRegistry.svelte';

const UNREAD_PROJECTS_STORE_KEY = 'unread-projects';

interface ProjectState {
  unread: boolean;
  runningSessions: Set<string>; // Set of session IDs currently running in this project
}

class ProjectStateStore {
  // Use $state for Maps to track reactivity via version increments
  private states = $state<Map<string, ProjectState>>(new Map());

  // Track version for manual reactivity triggering
  private version = $state(0);

  /**
   * Get the state for a project without creating it
   * Used by query methods to avoid side effects
   */
  private getState(projectId: string): ProjectState | undefined {
    return this.states.get(projectId);
  }

  /**
   * Get the state for a project, creating it if it doesn't exist
   * Only used by mutation methods that intentionally create state
   */
  private getOrCreateState(projectId: string): ProjectState {
    let state = this.getState(projectId);
    if (!state) {
      state = {
        unread: false,
        runningSessions: new Set(),
      };
      this.states.set(projectId, state);
      this.version++; // Trigger reactivity
    }
    return state;
  }

  /**
   * Check if a project is unread
   * Returns false for projects that don't exist yet (no side effects)
   * This getter accesses reactive state, so it will trigger re-renders when the state changes
   */
  isUnread(projectId: string): boolean {
    // Intentionally access version to establish a reactive dependency that will cause re-renders
    // when version changes (which happens whenever the project state is modified)
    this.version;
    return this.getState(projectId)?.unread ?? false;
  }

  /**
   * Check if a project has any running sessions
   * Returns false for projects that don't exist yet (no side effects)
   * This getter accesses reactive state, so it will trigger re-renders when the state changes
   */
  hasRunningSessions(projectId: string): boolean {
    // Intentionally access version to establish a reactive dependency that will cause re-renders
    // when version changes (which happens whenever the project state is modified)
    this.version;
    const state = this.getState(projectId);
    return state ? state.runningSessions.size > 0 : false;
  }

  /**
   * Mark a project as read (called when user navigates to the project)
   */
  markAsRead(projectId: string): void {
    const state = this.getOrCreateState(projectId);
    state.unread = false;
    this.version++; // Trigger reactivity
    this.persistAndBadge();
  }

  /**
   * Mark a project as unread (called when a session ends in that project
   * while the user is viewing a different project)
   */
  markAsUnread(projectId: string): void {
    const state = this.getOrCreateState(projectId);
    state.unread = true;
    this.version++; // Trigger reactivity
    this.persistAndBadge();
  }

  /**
   * Add a running session to a project
   * Note: Session registration is now handled by sessionRegistry,
   * this method only updates the project-level running session set
   */
  addRunningSession(projectId: string, sessionId: string): void {
    const state = this.getOrCreateState(projectId);
    const wasAlreadyAdded = state.runningSessions.has(sessionId);
    state.runningSessions.add(sessionId);
    // Only increment version if this is a new session to avoid unnecessary re-renders
    if (!wasAlreadyAdded) {
      this.version++;
    }
  }

  /**
   * Remove a running session from a project
   */
  removeRunningSession(projectId: string, sessionId: string): void {
    const state = this.states.get(projectId);
    if (state) {
      state.runningSessions.delete(sessionId);
      this.version++; // Trigger reactivity
    }
  }

  /**
   * Get the project ID for a session
   * Delegates to the unified session registry
   */
  getProjectForSession(sessionId: string): string | null {
    return sessionRegistry.getProjectId(sessionId);
  }

  /**
   * Handle session completion - mark project as unread if user is not viewing it
   * @param sessionId The session that completed
   * @param currentProjectId The project the user is currently viewing (or null if on projects list)
   */
  handleSessionComplete(sessionId: string, currentProjectId: string | null): void {
    const projectId = sessionRegistry.getProjectId(sessionId);
    if (!projectId) {
      // Sessions not in registry are expected - these are sessions that were started
      // before the registry was initialized or in other edge cases. We simply skip handling them.
      return;
    }

    // Remove the session from the project
    this.removeRunningSession(projectId, sessionId);

    // Mark as unread if user is not viewing this project
    if (currentProjectId !== projectId) {
      this.markAsUnread(projectId);
    }
  }

  /**
   * Clear all running sessions for a project
   */
  clearRunningSessions(projectId: string): void {
    const state = this.states.get(projectId);
    if (state) {
      state.runningSessions.clear();
      this.version++; // Trigger reactivity
    }
  }

  /**
   * Get the number of running sessions in a project
   */
  getRunningSessionCount(projectId: string): number {
    const state = this.states.get(projectId);
    return state ? state.runningSessions.size : 0;
  }

  /**
   * Get the session types of all running sessions for a project.
   * Returns an array of SessionType values (one per running session).
   * Delegates type lookups to the unified session registry.
   */
  getRunningSessionTypes(projectId: string): SessionType[] {
    // Access version to establish reactive dependency
    this.version;
    const state = this.getState(projectId);
    if (!state) return [];
    const types: SessionType[] = [];
    for (const sessionId of state.runningSessions) {
      const type = sessionRegistry.getType(sessionId);
      if (type) {
        types.push(type);
      }
    }
    return types;
  }

  /**
   * Get the number of projects currently in the unread state.
   */
  getUnreadCount(): number {
    this.version;
    let count = 0;
    for (const state of this.states.values()) {
      if (state.unread) count++;
    }
    return count;
  }

  /**
   * Persist the current set of unread project IDs to disk and update the dock badge.
   */
  private persistAndBadge(): void {
    const ids: string[] = [];
    for (const [id, state] of this.states) {
      if (state.unread) ids.push(id);
    }
    setStoreValue(UNREAD_PROJECTS_STORE_KEY, ids);
    getCurrentWindow()
      .setBadgeCount(ids.length || null)
      .catch(() => {
        // setBadgeCount may be unsupported on some platforms — ignore
      });
  }

  /**
   * Restore unread state from the persistent store and sync the badge.
   * Must be called after initPersistentStore().
   */
  async initFromStore(): Promise<void> {
    const ids = await getStoreValue<string[]>(UNREAD_PROJECTS_STORE_KEY);
    if (ids && ids.length > 0) {
      for (const id of ids) {
        const state = this.getOrCreateState(id);
        state.unread = true;
      }
      this.version++;
      getCurrentWindow()
        .setBadgeCount(ids.length)
        .catch(() => {});
    }
  }

  /**
   * Remove unread entries for projects that no longer exist.
   * Called during navigation init after the project list is fetched.
   */
  pruneDeletedProjects(existingProjectIds: Set<string>): void {
    let pruned = false;
    for (const [id, state] of this.states) {
      if (state.unread && !existingProjectIds.has(id)) {
        state.unread = false;
        pruned = true;
      }
    }
    if (pruned) {
      this.version++;
      this.persistAndBadge();
    }
  }
}

export const projectStateStore = new ProjectStateStore();
