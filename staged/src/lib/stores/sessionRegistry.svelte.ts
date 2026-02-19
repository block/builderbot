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
 */

export type SessionType = 'commit' | 'pr' | 'push' | 'note' | 'review' | 'other';

interface SessionMetadata {
  sessionId: string;
  projectId: string;
  branchId?: string; // Optional: only PR and push sessions are tied to a specific branch
  type: SessionType;
  timestamp: number; // When the session was registered
}

const MAX_REGISTRY_SIZE = 200; // Maximum number of sessions to track
const SESSION_TTL_MS = 48 * 60 * 60 * 1000; // 48 hours - keep longer than prState TTL
const CLEANUP_THRESHOLD = 0.8; // Run cleanup when registry is 80% full

class SessionRegistry {
  // Map from session ID to session metadata
  private sessions = $state<Map<string, SessionMetadata>>(new Map());

  // Track version for manual reactivity triggering (if needed for derived state)
  private version = $state(0);

  /**
   * Register a new session with its metadata
   */
  register(sessionId: string, projectId: string, type: SessionType, branchId?: string): void {
    // Only cleanup when we're approaching the size limit to avoid O(n) cost on every registration
    if (this.sessions.size >= MAX_REGISTRY_SIZE * CLEANUP_THRESHOLD) {
      this.cleanup();
    }

    console.info(
      `[sessionRegistry] register: session=${sessionId} project=${projectId} branch=${branchId ?? 'none'} type=${type} (size: ${this.sessions.size + 1})`
    );

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
    const meta = this.sessions.get(sessionId);
    if (meta) {
      console.info(
        `[sessionRegistry] unregister: session=${sessionId} type=${meta.type} project=${meta.projectId} branch=${meta.branchId ?? 'none'} (size: ${this.sessions.size - 1})`
      );
    } else {
      console.info(
        `[sessionRegistry] unregister: session=${sessionId} NOT FOUND in registry (size: ${this.sessions.size})`
      );
    }
    this.sessions.delete(sessionId);
    this.version++;
  }

  /**
   * Clean up stale sessions to prevent memory leaks
   * Removes sessions older than SESSION_TTL_MS or beyond MAX_REGISTRY_SIZE
   */
  private cleanup(): void {
    const now = Date.now();

    // Remove stale entries (older than TTL)
    for (const [sessionId, metadata] of this.sessions.entries()) {
      if (now - metadata.timestamp > SESSION_TTL_MS) {
        this.sessions.delete(sessionId);
      }
    }

    // If still over limit, remove oldest entries
    if (this.sessions.size > MAX_REGISTRY_SIZE) {
      const entries = Array.from(this.sessions.entries());
      entries.sort((a, b) => a[1].timestamp - b[1].timestamp);
      const toRemove = entries.slice(0, entries.length - MAX_REGISTRY_SIZE);
      for (const [sessionId] of toRemove) {
        this.sessions.delete(sessionId);
      }
    }
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
