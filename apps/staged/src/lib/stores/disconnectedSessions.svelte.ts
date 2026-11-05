/**
 * Disconnected Blox sessions, as this client knows them.
 *
 * A `disconnected` session lost its connection mid-turn: its agent may still
 * be working on the workstation, but nothing in this process is attached to
 * it, so it has no place in the session registry's live busy state. The
 * surfaces that still need to show it — the projects sidebar and grid —
 * read it from here instead, along with where the backend's reconcile service
 * stands on it.
 *
 * Two independent projections of backend state, both fed by
 * `sessionStatusListener`:
 *
 * - **Which sessions are disconnected**, per project: from
 *   `session-status-changed` (`disconnected` adds, any other status removes)
 *   and the `get_active_sessions` snapshot, which is authoritative for the set.
 * - **Reconcile state**, per session: from `session-reconcile-changed`
 *   (`idle`, the last event for a settled row, removes) and the
 *   `get_session_reconcile_states` snapshot.
 *
 * The timeline and chat pane read the row's own status for whether a session
 * is disconnected; they come here only for the reconcile state.
 */

import type { SessionReconcileEvent } from '../types';
import { representativeReconcileState } from '../shared/disconnectedSession';

interface DisconnectedEntry {
  projectId: string | null;
  branchId: string | null;
}

export interface DisconnectedSnapshotEntry {
  sessionId: string;
  projectId: string | null;
  branchId: string | null;
}

class DisconnectedSessionsStore {
  private sessions = $state<Map<string, DisconnectedEntry>>(new Map());
  private reconcile = $state<Map<string, SessionReconcileEvent>>(new Map());
  private version = $state(0);

  /** A `disconnected` status event, or a snapshot entry. */
  markDisconnected(sessionId: string, projectId: string | null, branchId: string | null): void {
    const current = this.sessions.get(sessionId);
    // A later event without a project keeps the one already learnt.
    const next = {
      projectId: projectId ?? current?.projectId ?? null,
      branchId: branchId ?? current?.branchId ?? null,
    };
    if (current?.projectId === next.projectId && current?.branchId === next.branchId) return;
    this.sessions.set(sessionId, next);
    this.version++;
  }

  /** The session left `disconnected` (any other status event). */
  clear(sessionId: string): void {
    if (this.sessions.delete(sessionId)) this.version++;
  }

  /**
   * Adopt the snapshot's disconnected rows as the whole set. `skip` names
   * sessions whose status changed while the snapshot was in flight: what this
   * client learnt about them is newer, so they are left as they are.
   */
  applySnapshot(entries: DisconnectedSnapshotEntry[], skip: (sessionId: string) => boolean): void {
    const snapshotIds = new Set(entries.map((entry) => entry.sessionId));
    let changed = false;
    for (const sessionId of [...this.sessions.keys()]) {
      if (!snapshotIds.has(sessionId) && !skip(sessionId)) {
        this.sessions.delete(sessionId);
        changed = true;
      }
    }
    if (changed) this.version++;
    for (const entry of entries) {
      if (skip(entry.sessionId)) continue;
      this.markDisconnected(entry.sessionId, entry.projectId, entry.branchId);
    }
  }

  /** Whether this client knows the session as disconnected. */
  isDisconnected(sessionId: string): boolean {
    this.version;
    return this.sessions.has(sessionId);
  }

  /** The project's disconnected sessions, in the order they were learnt. */
  sessionsForProject(projectId: string): string[] {
    this.version;
    const ids: string[] = [];
    for (const [sessionId, entry] of this.sessions) {
      if (entry.projectId === projectId) ids.push(sessionId);
    }
    return ids;
  }

  hasDisconnectedSessions(projectId: string): boolean {
    return this.sessionsForProject(projectId).length > 0;
  }

  /** A `session-reconcile-changed` event. */
  applyReconcileEvent(event: SessionReconcileEvent): void {
    if (event.phase === 'idle') {
      if (this.reconcile.delete(event.sessionId)) this.version++;
    } else {
      this.reconcile.set(event.sessionId, event);
      this.version++;
    }
    if (event.projectId && this.sessions.has(event.sessionId)) {
      this.markDisconnected(event.sessionId, event.projectId, event.branchId);
    }
  }

  /**
   * Adopt the `get_session_reconcile_states` snapshot. `skip` names sessions
   * an event arrived for while it was in flight; the event is newer.
   */
  applyReconcileSnapshot(
    events: SessionReconcileEvent[],
    skip: (sessionId: string) => boolean
  ): void {
    const snapshotIds = new Set(events.map((event) => event.sessionId));
    for (const sessionId of [...this.reconcile.keys()]) {
      if (!snapshotIds.has(sessionId) && !skip(sessionId)) {
        this.reconcile.delete(sessionId);
        this.version++;
      }
    }
    for (const event of events) {
      if (!skip(event.sessionId)) this.applyReconcileEvent(event);
    }
  }

  /** The reconcile state that speaks for a project's disconnected sessions. */
  projectReconcileState(projectId: string): SessionReconcileEvent | null {
    return representativeReconcileState(
      this.sessionsForProject(projectId).map((sessionId) => this.reconcile.get(sessionId))
    );
  }

  /** Where reconciling the session stands; null when nothing is known. */
  getReconcileState(sessionId: string | null | undefined): SessionReconcileEvent | null {
    this.version;
    if (!sessionId) return null;
    return this.reconcile.get(sessionId) ?? null;
  }
}

export const disconnectedSessionsStore = new DisconnectedSessionsStore();
