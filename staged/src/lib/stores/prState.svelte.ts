/**
 * Global PR creation state store
 * Persists PR creation state across navigation/component remounting
 */

export type PrState = 'idle' | 'creating' | 'error' | 'created';

interface BranchPrState {
  state: PrState;
  sessionId: string | null;
  error: string | null;
  url: string | null;
}

class PrStateStore {
  private states = $state<Map<string, BranchPrState>>(new Map());

  getPrState(branchId: string): BranchPrState | null {
    return this.states.get(branchId) ?? null;
  }

  setPrCreating(branchId: string, sessionId: string): void {
    this.states.set(branchId, {
      state: 'creating',
      sessionId,
      error: null,
      url: null,
    });
  }

  setPrCreated(branchId: string, url: string | null = null): void {
    this.states.set(branchId, {
      state: 'created',
      sessionId: null,
      error: null,
      url,
    });
  }

  setPrError(branchId: string, error: string): void {
    const existing = this.states.get(branchId);
    this.states.set(branchId, {
      state: 'error',
      sessionId: existing?.sessionId ?? null,
      error,
      url: null,
    });
  }

  clearPrState(branchId: string): void {
    this.states.delete(branchId);
  }

  getSessionId(branchId: string): string | null {
    return this.states.get(branchId)?.sessionId ?? null;
  }
}

export const prStateStore = new PrStateStore();
