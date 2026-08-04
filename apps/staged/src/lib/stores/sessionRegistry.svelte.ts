/**
 * Unified session registry
 *
 * Centralizes session tracking to eliminate redundant reverse lookups
 * across projectState and prState stores. Provides a single source of truth
 * for session-to-entity mappings.
 *
 * Design rationale:
 * - Both projectState and prState need to look up sessions (sessionId → projectId/branchId)
 * - Maintaining two separate maps leads to duplication and potential inconsistencies
 * - This registry provides symmetric tracking and cleanup for all session types
 *
 * The stores still maintain their distinct responsibilities:
 * - projectState: Aggregate project-level state (unread status, running session count)
 * - prState: Branch-specific PR workflow state (creating/created/error, PR URL)
 *
 * But they delegate session metadata tracking to this central registry.
 *
 * The registry is a pure projection of backend busy state: entries are added
 * by `session-status-changed` running events, launch-site registrations, and
 * the `get_active_sessions` snapshot hydration, and removed only on terminal
 * events or a hydration sweep. There is deliberately no client-side TTL or
 * size eviction — the backend guarantees terminal events via its session
 * state machine plus dead-session recovery, and local eviction is precisely
 * what would turn a missed event into a permanent lie.
 */

import { projectStateStore } from './projectState.svelte';

export type SessionType = 'commit' | 'pr' | 'push' | 'pull' | 'note' | 'review' | 'other';

interface SessionMetadata {
  sessionId: string;
  projectId: string;
  branchId?: string; // Optional: only PR, push, and pull sessions are tied to a specific branch
  type: SessionType;
  timestamp: number; // When the session was registered
}

class SessionRegistry {
  // Map from session ID to session metadata
  private sessions = $state<Map<string, SessionMetadata>>(new Map());

  // Track version for manual reactivity triggering (if needed for derived state)
  private version = $state(0);

  /**
   * Register a new session with its metadata
   */
  register(sessionId: string, projectId: string, type: SessionType, branchId?: string): void {
    this.sessions.set(sessionId, {
      sessionId,
      projectId,
      branchId,
      type,
      timestamp: Date.now(),
    });

    this.version++;
  }

  /**
   * Get the project ID for a session
   */
  getProjectId(sessionId: string): string | null {
    return this.sessions.get(sessionId)?.projectId ?? null;
  }

  /**
   * Get the branch ID for a session (returns null if session doesn't have a branch)
   */
  getBranchId(sessionId: string): string | null {
    return this.sessions.get(sessionId)?.branchId ?? null;
  }

  /**
   * Get the session type
   */
  getType(sessionId: string): SessionType | null {
    return this.sessions.get(sessionId)?.type ?? null;
  }

  /**
   * Get full metadata for a session
   */
  getMetadata(sessionId: string): SessionMetadata | null {
    return this.sessions.get(sessionId) ?? null;
  }

  /**
   * Find all sessions for a given project
   */
  getSessionsForProject(projectId: string): string[] {
    const sessionIds: string[] = [];
    for (const [sessionId, metadata] of this.sessions.entries()) {
      if (metadata.projectId === projectId) {
        sessionIds.push(sessionId);
      }
    }
    return sessionIds;
  }

  /**
   * Find all sessions for a given branch
   */
  getSessionsForBranch(branchId: string): string[] {
    const sessionIds: string[] = [];
    for (const [sessionId, metadata] of this.sessions.entries()) {
      if (metadata.branchId === branchId) {
        sessionIds.push(sessionId);
      }
    }
    return sessionIds;
  }

  /**
   * Find sessions by type (e.g., all PR sessions)
   */
  getSessionsByType(type: SessionType): string[] {
    const sessionIds: string[] = [];
    for (const [sessionId, metadata] of this.sessions.entries()) {
      if (metadata.type === type) {
        sessionIds.push(sessionId);
      }
    }
    return sessionIds;
  }

  /**
   * Unregister a session (called when session completes or is cleaned up)
   */
  unregister(sessionId: string): void {
    this.sessions.delete(sessionId);
    this.version++;
  }

  /**
   * Clean up a session's running state from projectStateStore and unregister it.
   *
   * This is the symmetric counterpart to register() — it removes the session
   * from both the project-level running session tracking and this registry.
   * Idempotent: safe to call even if the session is already gone.
   */
  cleanupSession(sessionId: string): void {
    const projectId = this.getProjectId(sessionId);
    if (projectId) {
      projectStateStore.removeRunningSession(projectId, sessionId);
    }
    this.unregister(sessionId);
  }

  /**
   * Get all tracked session IDs. Used by snapshot hydration to sweep entries
   * the backend no longer reports as active.
   */
  getAllSessionIds(): string[] {
    return Array.from(this.sessions.keys());
  }

  /**
   * Get the current number of tracked sessions
   */
  size(): number {
    return this.sessions.size;
  }

  /**
   * Clear all sessions (useful for testing or cleanup)
   */
  clear(): void {
    this.sessions.clear();
    this.version++;
  }
}

export const sessionRegistry = new SessionRegistry();
