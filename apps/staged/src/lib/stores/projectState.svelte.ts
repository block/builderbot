/**
 * Global project state store
 * Tracks which projects have running sessions and unread status
 *
 * A project is marked as "unread" when:
 * - A session completes in that project while the user is viewing a different project
 * - The user marks it, or one of its notes, unread
 *
 * A project is marked as "read" when:
 * - The user navigates to that project
 * - The user reads again, or deletes, every note whose "Mark as unread" flagged it
 *
 * Running sessions are tracked to show spinners on project cards
 * Unread state is persisted to disk and the macOS dock badge is kept in sync.
 *
 * Each window keeps its own unread flags, but the notes holding a project
 * unread are shared: every window sees a note's "Mark as unread", and reading
 * the note in any window releases it everywhere.
 *
 * Note: Session-to-project lookups are now delegated to the unified sessionRegistry
 */

import { getWindowSync } from '../transport';
import { getStoreValue, setStoreValue } from '../shared/persistentStore';
import { syncedStoreValue } from '../shared/syncedStoreValue';
import { sessionRegistry, type SessionType } from './sessionRegistry.svelte';

const UNREAD_PROJECTS_STORE_KEY = 'unread-projects';
/** Project id -> keys of the notes holding it unread; see `noteHolds`. */
const NOTE_UNREAD_HOLDS_STORE_KEY = 'note-unread-holds';

/** Persisted note holds, minus anything malformed and any project left with none. */
function normalizeNoteHolds(value: unknown): Map<string, Set<string>> {
  const holds = new Map<string, Set<string>>();
  if (!value || typeof value !== 'object' || Array.isArray(value)) return holds;
  for (const [projectId, noteKeys] of Object.entries(value)) {
    if (!Array.isArray(noteKeys)) continue;
    const held = new Set(noteKeys.filter((key): key is string => typeof key === 'string'));
    if (held.size > 0) holds.set(projectId, held);
  }
  return holds;
}

function serializeNoteHolds(holds: Map<string, Set<string>>): Record<string, string[]> {
  return Object.fromEntries([...holds].map(([projectId, held]) => [projectId, [...held]]));
}

interface ProjectState {
  /** Flagged in this window: by a session, the sidebar, or a restore. */
  unread: boolean;
  runningSessions: Set<string>; // Set of session IDs currently running in this project
  // Subset of runningSessions held open waiting on background work
  // (`session-background-hold`). Always pruned together with runningSessions.
  heldSessions: Set<string>;
}

class ProjectStateStore {
  // Use $state for Maps to track reactivity via version increments
  private states = $state<Map<string, ProjectState>>(new Map());

  // Track version for manual reactivity triggering
  private version = $state(0);

  /**
   * Project id -> keys of the notes whose "Mark as unread" holds the project
   * unread, until the user reads (or deletes) them all again or opens the
   * project. Separate from the window's own flag, which no note read clears.
   * Shared by every window through the persistent store, so a note marked
   * unread in one window flags its project in all of them, and reading it in
   * any window releases it everywhere. Never holds an empty set.
   */
  private noteHolds = new Map<string, Set<string>>();

  private noteHoldsSync = syncedStoreValue(NOTE_UNREAD_HOLDS_STORE_KEY, (value) => {
    this.noteHolds = normalizeNoteHolds(value);
    this.version++; // Trigger reactivity
  });

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
        heldSessions: new Set(),
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
    return (this.getState(projectId)?.unread ?? false) || this.noteHolds.has(projectId);
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
   * Mark a project as read (called when user navigates to the project).
   * Drops the notes holding it unread too, in every window.
   */
  markAsRead(projectId: string): void {
    if (this.noteHolds.delete(projectId)) this.saveNoteHolds();
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
   * Mark a project unread because the user marked one of its notes unread.
   * Reading that note again (`releaseNoteUnread`) releases the hold, which
   * clears the project once no other note holds it — though a project this
   * window flagged some other way stays flagged here.
   */
  markUnreadForNote(projectId: string, noteKey: string): void {
    const held = this.noteHolds.get(projectId);
    if (held?.has(noteKey)) return;
    if (held) held.add(noteKey);
    else this.noteHolds.set(projectId, new Set([noteKey]));
    this.noteHoldsChanged();
  }

  /** The projects the note's "Mark as unread" helps hold unread. */
  projectsFlaggedByNote(noteKey: string): string[] {
    const projectIds: string[] = [];
    for (const [projectId, held] of this.noteHolds) {
      if (held.has(noteKey)) projectIds.push(projectId);
    }
    return projectIds;
  }

  /** The user read a note whose "Mark as unread" held the project unread. */
  releaseNoteUnread(projectId: string, noteKey: string): void {
    if (this.releaseHold(projectId, noteKey)) this.noteHoldsChanged();
  }

  /**
   * The note was deleted, so nothing will read it again: releases its hold on
   * every project its "Mark as unread" flagged, as reading it would have.
   */
  forgetNote(noteKey: string): void {
    let changed = false;
    for (const projectId of this.projectsFlaggedByNote(noteKey)) {
      changed = this.releaseHold(projectId, noteKey) || changed;
    }
    if (changed) this.noteHoldsChanged();
  }

  private releaseHold(projectId: string, noteKey: string): boolean {
    const held = this.noteHolds.get(projectId);
    if (!held?.delete(noteKey)) return false;
    if (held.size === 0) this.noteHolds.delete(projectId);
    return true;
  }

  private noteHoldsChanged(): void {
    this.version++; // Trigger reactivity
    this.saveNoteHolds();
    this.syncBadge();
  }

  private saveNoteHolds(): void {
    this.noteHoldsSync.save(serializeNoteHolds(this.noteHolds));
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
      state.heldSessions.delete(sessionId);
      this.version++; // Trigger reactivity
    }
  }

  /**
   * Record whether a running session is held open waiting on background work
   * (`session-background-hold`). A hold is a sub-state of running: it is only
   * recorded for a session this project is tracking as running, and leaves
   * with it.
   */
  setSessionBackgroundHold(projectId: string, sessionId: string, holding: boolean): void {
    const state = this.getState(projectId);
    if (!state || !state.runningSessions.has(sessionId)) return;
    const wasHeld = state.heldSessions.has(sessionId);
    if (holding === wasHeld) return;
    if (holding) {
      state.heldSessions.add(sessionId);
    } else {
      state.heldSessions.delete(sessionId);
    }
    this.version++;
  }

  /**
   * Check whether every running session in a project is waiting on background
   * work, i.e. the project is busy but nothing visible is happening.
   * False for projects with no running sessions.
   */
  isWaitingOnBackground(projectId: string): boolean {
    this.version;
    const state = this.getState(projectId);
    if (!state || state.runningSessions.size === 0) return false;
    for (const sessionId of state.runningSessions) {
      if (!state.heldSessions.has(sessionId)) return false;
    }
    return true;
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
      state.heldSessions.clear();
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
    return this.countUnread();
  }

  /** `getUnreadCount` without the reactive read, for mutations to badge with. */
  private countUnread(): number {
    const ids = new Set(this.noteHolds.keys());
    for (const [id, state] of this.states) {
      if (state.unread) ids.add(id);
    }
    return ids.size;
  }

  /**
   * Persist this window's unread flags to disk and update the dock badge. The
   * note holds persist on their own (`saveNoteHolds`).
   */
  private persistAndBadge(): void {
    const ids: string[] = [];
    for (const [id, state] of this.states) {
      if (state.unread) ids.push(id);
    }
    setStoreValue(UNREAD_PROJECTS_STORE_KEY, ids);
    this.syncBadge();
  }

  private syncBadge(): void {
    getWindowSync()
      .setBadgeCount(this.countUnread() || undefined)
      .catch(() => {
        // setBadgeCount may be unsupported on some platforms — ignore
      });
  }

  /**
   * Restore unread state from the persistent store, start following the note
   * holds other windows write, and sync the badge.
   * Must be called after initPersistentStore().
   */
  async initFromStore(): Promise<void> {
    const [ids] = await Promise.all([
      getStoreValue<string[]>(UNREAD_PROJECTS_STORE_KEY),
      this.noteHoldsSync.start(),
    ]);
    if (ids && ids.length > 0) {
      for (const id of ids) {
        const state = this.getOrCreateState(id);
        state.unread = true;
      }
      this.version++;
    }
    if (this.countUnread() > 0) this.syncBadge();
  }

  /** Stop following other windows' note holds. */
  dispose(): void {
    this.noteHoldsSync.dispose();
  }

  /**
   * Remove unread entries for projects that no longer exist.
   * Operates directly on persisted data so it works regardless of
   * whether initFromStore() has run yet.
   */
  async pruneDeletedProjects(existingProjectIds: Set<string>): Promise<void> {
    const [persistedIds, persistedHolds] = await Promise.all([
      getStoreValue<string[]>(UNREAD_PROJECTS_STORE_KEY),
      getStoreValue<unknown>(NOTE_UNREAD_HOLDS_STORE_KEY),
    ]);
    const ids = persistedIds ?? [];
    const holds = normalizeNoteHolds(persistedHolds);
    const deletedIds = ids.filter((id) => !existingProjectIds.has(id));
    const deletedHolders = [...holds.keys()].filter((id) => !existingProjectIds.has(id));
    if (deletedIds.length === 0 && deletedHolders.length === 0) return;

    if (deletedIds.length > 0) {
      await setStoreValue(
        UNREAD_PROJECTS_STORE_KEY,
        ids.filter((id) => existingProjectIds.has(id))
      );
    }
    if (deletedHolders.length > 0) {
      for (const id of deletedHolders) holds.delete(id);
      // Through the sync, so every window drops them too.
      this.noteHoldsSync.save(serializeNoteHolds(holds));
    }

    // Also update in-memory state if it has been initialized
    for (const id of deletedIds) {
      const state = this.states.get(id);
      if (state) state.unread = false;
    }
    for (const id of deletedHolders) this.noteHolds.delete(id);
    this.version++;
    this.syncBadge();
  }
}

export const projectStateStore = new ProjectStateStore();

if (import.meta.hot) {
  import.meta.hot.dispose(() => projectStateStore.dispose());
}
