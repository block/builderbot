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
  private states = $state<Map<string, ProjectState>>(new Map());
  // Map from session ID to project ID to track which project a session belongs to
  private sessionToProject = $state<Map<string, string>>(new Map());

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
    }
    return state;
  }

  /**
   * Check if a project is unread
   */
  isUnread(projectId: string): boolean {
    return this.states.get(projectId)?.unread ?? false;
  }

  /**
   * Check if a project has any running sessions
   */
  hasRunningSessions(projectId: string): boolean {
    const state = this.states.get(projectId);
    return state ? state.runningSessions.size > 0 : false;
  }

  /**
   * Mark a project as read (called when user navigates to the project)
   */
  markAsRead(projectId: string): void {
    const state = this.getOrCreateState(projectId);
    state.unread = false;
  }

  /**
   * Mark a project as unread (called when a session ends in that project
   * while the user is viewing a different project)
   */
  markAsUnread(projectId: string): void {
    const state = this.getOrCreateState(projectId);
    state.unread = true;
  }

  /**
   * Add a running session to a project
   */
  addRunningSession(projectId: string, sessionId: string): void {
    const state = this.getOrCreateState(projectId);
    state.runningSessions.add(sessionId);
    // Track the session-to-project mapping
    this.sessionToProject.set(sessionId, projectId);
  }

  /**
   * Remove a running session from a project
   */
  removeRunningSession(projectId: string, sessionId: string): void {
    const state = this.states.get(projectId);
    if (state) {
      state.runningSessions.delete(sessionId);
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
    if (!projectId) return;

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
