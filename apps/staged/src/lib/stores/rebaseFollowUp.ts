/**
 * Force pushes waiting on a rebase.
 *
 * "Rebase and force push" is two pipelines, not one: the rebase runs (or
 * queues) exactly as a plain rebase does, and the force push is only requested
 * once that rebase session ends successfully. Queueing the push up front would
 * run it after a failed rebase too, which is the one outcome the user picked
 * this action to avoid.
 *
 * The entry is keyed by branch and pinned to the rebase session id so a later,
 * unrelated commit session on the same branch can't trigger it. It lives in
 * memory only: a follow-up lost to an app restart mid-rebase just means the
 * user force pushes by hand, which is what they'd have done anyway.
 */

export interface RebaseFollowUp {
  /** The rebase pipeline session whose success releases the force push. */
  rebaseSessionId: string;
  /** Agent provider the rebase was requested with; reused for the push. */
  provider?: string;
}

const followUps = new Map<string, RebaseFollowUp>();

export const rebaseFollowUpStore = {
  /** Arm a force push behind the given rebase session. Replaces any earlier one. */
  set(branchId: string, followUp: RebaseFollowUp): void {
    followUps.set(branchId, followUp);
  },

  /**
   * Consume the follow-up armed behind `rebaseSessionId`, or `null` when the
   * ending session isn't the one it waits on.
   */
  take(branchId: string, rebaseSessionId: string): RebaseFollowUp | null {
    const followUp = followUps.get(branchId);
    if (!followUp || followUp.rebaseSessionId !== rebaseSessionId) return null;
    followUps.delete(branchId);
    return followUp;
  },

  clear(branchId: string): void {
    followUps.delete(branchId);
  },

  /** Test-only reset. */
  reset(): void {
    followUps.clear();
  },
};
