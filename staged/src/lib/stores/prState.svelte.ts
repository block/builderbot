/**
 * Global PR creation state store
 * Persists PR creation state across navigation/component remounting
 *
 * Note: The Map is wrapped in $state, and direct mutations (.set(), .delete())
 * trigger reactivity in Svelte 5. Cleanup is performed to prevent memory leaks
 * by removing stale states when branches are deleted or PRs are completed.
 *
 * Session lookups are now delegated to the unified sessionRegistry
 */

import { sessionRegistry } from './sessionRegistry.svelte';

export type PrState = 'idle' | 'creating' | 'error' | 'created';

interface BranchPrState {
  state: PrState;
  sessionId: string | null;
  error: string | null;
  url: string | null;
  timestamp: number; // Track when state was last updated
}

const MAX_STORE_SIZE = 100; // Maximum number of branch states to keep
const STATE_TTL_MS = 24 * 60 * 60 * 1000; // 24 hours
const CLEANUP_THRESHOLD = 0.8; // Run cleanup when store is 80% full

class PrStateStore {
  private states = $state<Map<string, BranchPrState>>(new Map());

  getPrState(branchId: string): BranchPrState | null {
    return this.states.get(branchId) ?? null;
  }

  setPrCreating(branchId: string, sessionId: string): void {
    console.info(`[session:prState] setPrCreating: branch=${branchId} session=${sessionId}`);
    // Only cleanup when approaching size limit to avoid O(n) cost on every operation
    if (this.states.size >= MAX_STORE_SIZE * CLEANUP_THRESHOLD) {
      this.cleanup();
    }
    this.states.set(branchId, {
      state: 'creating',
      sessionId,
      error: null,
      url: null,
      timestamp: Date.now(),
    });
  }

  setPrCreated(branchId: string, url: string | null = null): void {
    console.info(`[session:prState] setPrCreated: branch=${branchId} url=${url}`);
    if (this.states.size >= MAX_STORE_SIZE * CLEANUP_THRESHOLD) {
      this.cleanup();
    }
    this.states.set(branchId, {
      state: 'created',
      sessionId: null,
      error: null,
      url,
      timestamp: Date.now(),
    });
  }

  setPrError(branchId: string, error: string): void {
    console.info(`[session:prState] setPrError: branch=${branchId} error=${error}`);
    if (this.states.size >= MAX_STORE_SIZE * CLEANUP_THRESHOLD) {
      this.cleanup();
    }
    const existing = this.states.get(branchId);
    this.states.set(branchId, {
      state: 'error',
      sessionId: existing?.sessionId ?? null,
      error,
      url: null,
      timestamp: Date.now(),
    });
  }

  clearPrState(branchId: string): void {
    console.info(`[session:prState] clearPrState: branch=${branchId}`);
    this.states.delete(branchId);
  }

  /**
   * Clean up stale states to prevent memory leaks.
   * Removes states that are:
   * 1. Older than STATE_TTL_MS (24 hours)
   * 2. In 'created' state (already completed, no longer needed)
   * 3. Beyond MAX_STORE_SIZE limit (keeps most recent entries)
   */
  private cleanup(): void {
    const now = Date.now();

    // Remove stale entries (older than TTL or completed PRs)
    for (const [branchId, state] of this.states.entries()) {
      if (now - state.timestamp > STATE_TTL_MS || state.state === 'created') {
        this.states.delete(branchId);
      }
    }

    // If still over limit, remove oldest entries
    if (this.states.size > MAX_STORE_SIZE) {
      const entries = Array.from(this.states.entries());
      entries.sort((a, b) => a[1].timestamp - b[1].timestamp);
      const toRemove = entries.slice(0, entries.length - MAX_STORE_SIZE);
      for (const [branchId] of toRemove) {
        this.states.delete(branchId);
      }
    }
  }

  getSessionId(branchId: string): string | null {
    return this.states.get(branchId)?.sessionId ?? null;
  }

  /**
   * Find the branch ID associated with a session ID
   * Used by the global session listener to update PR state when sessions complete
   * Delegates to the unified session registry, but only returns branch if this is a PR session
   */
  getBranchIdForSession(sessionId: string): string | null {
    const branchId = sessionRegistry.getBranchId(sessionId);
    if (!branchId) {
      return null;
    }

    // Only return the branch ID if we have a 'creating' state for it
    // This ensures we're only tracking PR sessions, not other branch-related sessions
    const state = this.states.get(branchId);
    if (state?.sessionId === sessionId && state.state === 'creating') {
      return branchId;
    }

    return null;
  }

  /**
   * Clear session tracking for a branch
   * Called after PR creation completes to remove the session association
   * Note: Does NOT unregister from sessionRegistry - that's handled centrally in App.svelte
   */
  clearSessionTracking(branchId: string): void {
    const state = this.states.get(branchId);
    if (state?.sessionId) {
      // Clear the session ID reference, but leave the final state (created/error)
      // This maintains the PR outcome while removing the transient session reference
      state.sessionId = null;
    }
  }
}

export const prStateStore = new PrStateStore();
