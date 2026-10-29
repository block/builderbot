/** What a queued row would occupy on the branch if it started. */
export type QueuedSessionKind = 'commit' | 'note' | 'review';

/**
 * Whether "Start now" should be offered on a queued session row.
 *
 * The action starts that specific row ahead of the queue, so the queue's own
 * rules — FIFO order, one running session per kind, waiting out a git action —
 * are not reasons to withhold it. Two reasons remain:
 *
 * - **Provisioning.** With no worktree yet (or a remote workspace still
 *   starting) the backend cannot resolve a working directory, so the start fails
 *   at the worktree lookup. Nothing to offer until it finishes, and the queue
 *   drains on its own the moment it does.
 * - **A writer owns the worktree.** A running commit session, rebase, or
 *   squash, or an in-flight push, pull, reset-to-origin, or discard, holds the
 *   worktree, so a queued commit (including a queued rebase or squash) is
 *   withheld: it would put a second writer in the tree. A pull, reset, or
 *   discard moves the tree itself. A push only reads it, but a failed push — a
 *   pre-push hook checking the very tree the commit would be editing, say — is
 *   handed to an agent that edits the tree to fix it (all but a plain push's
 *   non-fast-forward rejection). Notes and reviews are offered beside all of
 *   them: starting one beside a running writer is the action's main use case,
 *   and reading a tree mid-edit is the trade it makes.
 *
 * Reset-to-origin and discard run outside the session queue, and the flags
 * behind `gitActionRunning` only know about the ones this client started — so
 * this check is the fast local withhold, not the safety net. The backend marks
 * the branch while either runs and refuses a forced commit from any connected
 * client, which is what catches a reset or discard another window started.
 *
 * The item is omitted rather than rendered disabled: the row already reads as
 * queued, and it comes back on its own once the branch frees up.
 */
export function canStartQueuedSessionNow(args: {
  /** Which kind of session the queued row would start. */
  kind: QueuedSessionKind;
  /** A commit session, rebase, or squash is running on the branch. */
  commitSessionRunning: boolean;
  /** A push, pull, reset-to-origin, or discard is in flight on the branch. */
  gitActionRunning: boolean;
  /** The branch has no usable worktree yet. */
  provisioning: boolean;
}): boolean {
  if (args.provisioning) return false;
  if (args.kind !== 'commit') return true;
  return !args.commitSessionRunning && !args.gitActionRunning;
}
