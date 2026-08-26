/**
 * Whether the Start button on a queued session row would actually start it.
 *
 * Start drains the branch's session queue, and the backend refuses to start
 * anything while another branch session holds the queue: a running push, force
 * push, or pull takes the branch exclusively, so every click during one is a
 * no-op. Those git sessions leave no timeline artifact either, so the
 * timeline's own "a session is running" check reads the branch as idle for
 * their whole duration and keeps offering a button that does nothing.
 *
 * Reset-to-origin and discard run outside the session queue, so a drain during
 * one would start — which is the reason to withhold the button rather than a
 * reason to keep it, since both rewrite the worktree the queued session is
 * about to run in.
 *
 * Rendering nothing beats rendering a disabled control here: the row already
 * reads as queued, and the button comes back on its own when the git action
 * finishes.
 */
export function canStartQueuedSessions(args: {
  /** A session that isn't itself queued is running on this branch. */
  hasActiveSession: boolean;
  /** A push, force push, pull, reset, or discard of the branch's own is running. */
  gitActionRunning: boolean;
}): boolean {
  return !args.hasActiveSession && !args.gitActionRunning;
}
