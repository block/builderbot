/**
 * Global pull state store.
 *
 * Only queued pulls live here. An immediate pull is a direct git operation the
 * branch card awaits inline, but a queued one outlives the click: it waits on the
 * branch session queue, then runs when the queue drains it. Keeping it in a store
 * (rather than in BranchCard state) means the "Queued" badge survives the remount
 * that happens when the user switches projects and back — the same reason
 * `pushState` exists.
 *
 * Like `pushState`, this is frontend-only: a pull that is still queued when the
 * app restarts drains without ever having been badged as queued.
 *
 * The Map is wrapped in $state, but $state(Map) does not give fine-grained
 * reactivity for .get()/.set(), so a version counter is bumped on every mutation
 * and read by `getPullState` to establish the dependency.
 */

/** `queued` flips to `pulling` when the branch queue drains the session. */
export type PullState = 'queued' | 'pulling';

interface BranchPullState {
  state: PullState;
  sessionId: string;
  timestamp: number;
}

const MAX_STORE_SIZE = 100;
const STATE_TTL_MS = 24 * 60 * 60 * 1000; // 24 hours
const CLEANUP_THRESHOLD = 0.8;

class PullStateStore {
  private states = $state<Map<string, BranchPullState>>(new Map());
  private version = $state(0);

  getPullState(branchId: string): BranchPullState | null {
    // Read the counter so $derived callers re-evaluate on any mutation.
    this.version;
    return this.states.get(branchId) ?? null;
  }

  /** Record a pull the backend put on the branch queue. */
  setPullQueued(branchId: string, sessionId: string): void {
    this.set(branchId, { state: 'queued', sessionId, timestamp: Date.now() });
  }

  /**
   * Flip a queued pull to `pulling` once the branch queue drains it.
   *
   * Guarded on the session ID so an unrelated pull event can't revive a stale
   * queued entry (e.g. one the user cancelled and re-requested).
   */
  markQueuedPullStarted(branchId: string, sessionId: string): void {
    const existing = this.states.get(branchId);
    if (existing?.state !== 'queued' || existing.sessionId !== sessionId) {
      return;
    }
    this.set(branchId, { state: 'pulling', sessionId, timestamp: Date.now() });
  }

  clearPullState(branchId: string): void {
    this.states.delete(branchId);
    this.version++;
  }

  private set(branchId: string, next: BranchPullState): void {
    if (this.states.size >= MAX_STORE_SIZE * CLEANUP_THRESHOLD) {
      this.cleanup();
    }
    this.states.set(branchId, next);
    this.version++;
  }

  /** Drop stale entries so a long-running app doesn't accumulate them. */
  private cleanup(): void {
    const now = Date.now();
    for (const [branchId, state] of this.states.entries()) {
      if (now - state.timestamp > STATE_TTL_MS) {
        this.states.delete(branchId);
      }
    }

    if (this.states.size > MAX_STORE_SIZE) {
      const entries = Array.from(this.states.entries());
      entries.sort((a, b) => a[1].timestamp - b[1].timestamp);
      for (const [branchId] of entries.slice(0, entries.length - MAX_STORE_SIZE)) {
        this.states.delete(branchId);
      }
    }
  }
}

export const pullStateStore = new PullStateStore();
