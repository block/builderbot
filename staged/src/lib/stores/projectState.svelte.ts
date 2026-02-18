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
 */

interface ProjectState {
  unread: boolean;
  runningSessions: Set<string>; // Set of session IDs currently running in this project
}

class ProjectStateStore {
  // Use $state.raw for Maps to avoid deep reactivity overhead while maintaining reactivity
  private states = $state<Map<string, ProjectState>>(new Map());
  // Map from session ID to project ID to track which project a session belongs to
  private sessionToProject = $state<Map<string, string>>(new Map());

  // Track version for manual reactivity triggering
  private version = $state(0);

  /**
   * Get the state for a project, creating it if it doesn't exist
   */
  private getOrCreateState(projectId: string): ProjectState {
    let state = this.states.get(projectId);
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
   * This getter accesses reactive state, so it will trigger re-renders when the state changes
   */
  isUnread(projectId: string): boolean {
    // Access version to ensure reactivity
    this.version;
    return this.states.get(projectId)?.unread ?? false;
  }

  /**
   * Check if a project has any running sessions
   * This getter accesses reactive state, so it will trigger re-renders when the state changes
   */
  hasRunningSessions(projectId: string): boolean {
    // Access version to ensure reactivity
    this.version;
    const state = this.states.get(projectId);
    return state ? state.runningSessions.size > 0 : false;
  }

  /**
   * Mark a project as read (called when user navigates to the project)
   */
  markAsRead(projectId: string): void {
    const state = this.getOrCreateState(projectId);
    state.unread = false;
    this.version++; // Trigger reactivity
  }

  /**
   * Mark a project as unread (called when a session ends in that project
   * while the user is viewing a different project)
   */
  markAsUnread(projectId: string): void {
    const state = this.getOrCreateState(projectId);
    state.unread = true;
    this.version++; // Trigger reactivity
  }

  /**
   * Add a running session to a project
   */
  addRunningSession(projectId: string, sessionId: string): void {
    const state = this.getOrCreateState(projectId);
    state.runningSessions.add(sessionId);
    // Track the session-to-project mapping
    this.sessionToProject.set(sessionId, projectId);
    this.version++; // Trigger reactivity
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
    // Clean up the session-to-project mapping
    this.sessionToProject.delete(sessionId);
  }

  /**
   * Get the project ID for a session
   */
  getProjectForSession(sessionId: string): string | null {
    return this.sessionToProject.get(sessionId) ?? null;
  }

  /**
   * Handle session completion - mark project as unread if user is not viewing it
   * @param sessionId The session that completed
   * @param currentProjectId The project the user is currently viewing (or null if on projects list)
   */
  handleSessionComplete(sessionId: string, currentProjectId: string | null): void {
    const projectId = this.sessionToProject.get(sessionId);
    if (!projectId) {
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
}

export const projectStateStore = new ProjectStateStore();
