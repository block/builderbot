/** What a queued row would occupy on the branch if it started. */
export type QueuedSessionKind = 'commit' | 'note' | 'review';

/**
 * Whether "Start now" should be offered on a queued session row.
 *
 * The action starts that specific row ahead of the queue, so the queue's own
 * rules — FIFO order, one running session per kind — are not reasons to withhold
 * it. Two reasons remain:
 *
 * - **Provisioning.** With no worktree yet (or a remote workspace still
 *   starting) the backend cannot resolve a working directory, so the start fails
 *   at the worktree lookup. Nothing to offer until it finishes, and the queue
 *   drains on its own the moment it does.
 * - **A worktree writer.** A queued commit (including a queued rebase or squash)
 *   would put a second agent in the tree a running commit session, push, pull,
 *   reset, or discard already owns. Notes and reviews only read, so they are
 *   offered throughout — starting one beside a running commit is the action's
 *   main use case, and reading a tree mid-edit is the trade it makes.
 *
 * Reset-to-origin and discard run outside the session queue, so the backend's
 * own guard cannot see them; this frontend check is the only thing withholding
 * the action during those.
 *
 * The item is omitted rather than rendered disabled: the row already reads as
 * queued, and it comes back on its own once the branch frees up.
 */
export function canStartQueuedSessionNow(args: {
  /** Which kind of session the queued row would start. */
  kind: QueuedSessionKind;
  /** A commit session, rebase, squash, push, pull, reset, or discard holds the worktree. */
  exclusiveHolderRunning: boolean;
  /** The branch has no usable worktree yet. */
  provisioning: boolean;
}): boolean {
  if (args.provisioning) return false;
  if (args.kind !== 'commit') return true;
  return !args.exclusiveHolderRunning;
}
